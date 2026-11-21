use super::fixtures::*;
use super::*;

const COMMITTED_BODY_SIZE: usize = 2 * SLOW_CHUNK_SIZE;

#[tokio::test]
async fn timeout_after_commit_does_not_retry_a_slow_upstream() {
    let Some(first_listener) = bind_backend("committed timeout first upstream").await else {
        return;
    };
    let Some(second_listener) = bind_backend("committed timeout retry upstream").await else {
        return;
    };
    let first_address = first_listener.local_addr().unwrap();
    let second_address = second_listener.local_addr().unwrap();

    let first = tokio::spawn(async move {
        let (mut stream, _) = first_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {SLOW_BODY_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream
            .write_all(&vec![b'x'; COMMITTED_BODY_SIZE])
            .await
            .unwrap();
        stream.flush().await.unwrap();
        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("read timeout should close the first upstream")
            .unwrap();
        assert_eq!(closed, 0);
    });
    let second = tokio::spawn(async move {
        let retry =
            tokio::time::timeout(Duration::from_millis(500), second_listener.accept()).await;
        assert!(
            retry.is_err(),
            "a response committed before the upstream timeout must not be retried"
        );
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp",
            &format!("server {first_address}; server {second_address};"),
            1000,
            "200ms",
            "proxy_next_upstream error timeout non_idempotent; \
             proxy_next_upstream_tries 2;",
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
        .write_all(b"GET /timeout HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    let response = read_to_end_within(&mut client, Duration::from_secs(3)).await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(!headers.contains("504"), "{headers}");
    assert_eq!(body, vec![b'x'; COMMITTED_BODY_SIZE]);

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("committed timeout should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("first upstream should close after the read timeout")
        .unwrap();
    second.await.unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_backend(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}
