use tokio::io::{AsyncBufReadExt, BufReader};

use super::fixtures::*;
use super::*;

#[tokio::test]
async fn ignore_client_abort_preserves_a_slow_response_after_a_half_close() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        read_request_head_and_body(&mut stream, b"body").await;

        stream
            .get_mut()
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {SLOW_BODY_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let chunk = vec![b'x'; SLOW_CHUNK_SIZE];
        for _ in 0..SLOW_BODY_SIZE / SLOW_CHUNK_SIZE {
            stream.get_mut().write_all(&chunk).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        stream.get_mut().flush().await.unwrap();

        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("half-closed client should close upstream after the response")
            .unwrap();
        assert_eq!(closed, 0);
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp",
            &format!("server {upstream_address};"),
            1000,
            "150ms",
            "proxy_ignore_client_abort on;",
        ),
        &[],
    );
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(
            b"POST /half-close HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\
              Connection: close\r\n\r\nbody",
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    let response = read_response_with_body(&mut client, SLOW_BODY_SIZE)
        .await
        .expect("ignore_client_abort should keep the response alive after a half-close");
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), SLOW_BODY_SIZE);
    assert!(body.iter().all(|byte| *byte == b'x'));

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("completed response should stop the half-closed client handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("slow upstream should complete after the client half-closes")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_head_and_body(stream: &mut BufReader<TcpStream>, expected_body: &[u8]) {
    let mut line = String::new();
    let mut content_length = None;
    loop {
        line.clear();
        let read = stream.read_line(&mut line).await.unwrap();
        assert!(read != 0, "upstream closed before request headers");
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    assert_eq!(content_length, Some(expected_body.len()));
    let mut body = vec![0_u8; expected_body.len()];
    stream.read_exact(&mut body).await.unwrap();
    assert_eq!(body, expected_body);
}

async fn bind_backend() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping slow-upstream half-close test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind slow-upstream half-close backend: {error}"),
    }
}
