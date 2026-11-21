use crate::server::run::reload;

use super::fixtures::*;
use super::*;

#[tokio::test]
async fn reload_discards_a_completed_old_generation_pool_connection() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut old_stream, _) = upstream.accept().await.unwrap();
        let head = read_request_head(&mut old_stream).await;
        assert!(
            head.starts_with("GET /old-slow HTTP/1.1\r\n"),
            "old generation request was not sent first:\n{head}"
        );
        write_slow_response(&mut old_stream).await;

        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(3), old_stream.read(&mut byte))
            .await
            .expect("old generation connection should be closed after reload");
        assert_eq!(
            closed.unwrap(),
            0,
            "old generation connection entered the new pool"
        );

        let (mut new_stream, _) = tokio::time::timeout(Duration::from_secs(5), upstream.accept())
            .await
            .expect("new generation should open a fresh upstream connection")
            .unwrap();
        let head = read_request_head(&mut new_stream).await;
        assert!(
            head.starts_with("GET /new HTTP/1.1\r\n"),
            "new generation reused the discarded connection:\n{head}"
        );
        write_probe_response(&mut new_stream, b"new-generation").await;
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp-old",
            &format!("server {upstream_address}; keepalive 2;"),
            1000,
            "150ms",
            "proxy_ignore_client_abort on;",
        ),
        &[],
    );
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");
    let old_temp_root = prefix.join("proxy-temp-old");

    let first_frontend = bind_frontend().await;
    let first_address = first_frontend.local_addr().unwrap();
    let first_server = spawn_frontend(first_frontend, &config, state.clone());
    let mut first_client = TcpStream::connect(first_address).await.unwrap();
    socket2::SockRef::from(&first_client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    first_client
        .write_all(b"GET /old-slow HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    first_client.shutdown().await.unwrap();

    wait_for_generated_temp_file(&old_temp_root, true).await;
    std::fs::write(
        &config_path,
        spill_config_with_named_upstream(
            "proxy-temp-new",
            &format!("server {upstream_address}; keepalive 2;"),
            1001,
            "150ms",
            "proxy_ignore_client_abort on;",
        ),
    )
    .unwrap();
    reload(&state, &config_path, &prefix).await;
    assert_eq!(
        state
            .config_snapshot()
            .await
            .config
            .http
            .as_ref()
            .unwrap()
            .servers[0]
            .context
            .client_max_body_size,
        1001
    );

    let old_response = read_response_with_body(&mut first_client, SLOW_BODY_SIZE)
        .await
        .expect("the old generation slow response should finish after reload");
    assert_complete_response(&old_response);
    tokio::time::timeout(Duration::from_secs(2), first_server)
        .await
        .expect("old generation client handler should stop after the response")
        .unwrap();
    wait_for_generated_temp_file(&old_temp_root, false).await;
    assert_eq!(
        state.upstream_keepalive.len("p0_backend").await,
        0,
        "a completed request must not return its old generation connection to the pool"
    );

    let second_frontend = bind_frontend().await;
    let second_address = second_frontend.local_addr().unwrap();
    let second_server = spawn_frontend(second_frontend, &config, state);
    let response = request_probe(second_address, "/new").await;
    assert_eq!(response, b"new-generation");
    tokio::time::timeout(Duration::from_secs(2), second_server)
        .await
        .expect("new generation request handler should stop after Connection: close")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("backend should observe old connection closure and a new connection")
        .unwrap();
    assert!(
        temp_files(&prefix.join("proxy-temp-new")).is_empty(),
        "old generation cleanup polluted the new temp root"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_head(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(request).expect("upstream request headers should be UTF-8")
}

async fn write_slow_response(stream: &mut TcpStream) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {SLOW_BODY_SIZE}\r\nConnection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let chunk = vec![b'x'; SLOW_CHUNK_SIZE];
    for _ in 0..SLOW_BODY_SIZE / SLOW_CHUNK_SIZE {
        stream.write_all(&chunk).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    stream.flush().await.unwrap();
}

async fn write_probe_response(stream: &mut TcpStream, body: &[u8]) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
    stream.flush().await.unwrap();
}

async fn request_probe(frontend: SocketAddr, path: &str) -> Vec<u8> {
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let response = read_to_end_within(&mut client, Duration::from_secs(2)).await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    body.to_vec()
}

fn assert_complete_response(response: &[u8]) {
    let (headers, body) = split_http_response(response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), SLOW_BODY_SIZE);
    assert!(body.iter().all(|byte| *byte == b'x'));
}

async fn bind_backend() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping generation pressure test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind generation pressure backend: {error}"),
    }
}
