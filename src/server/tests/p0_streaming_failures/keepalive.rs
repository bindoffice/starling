use super::fixtures::*;
use super::*;

#[tokio::test]
async fn slow_upstream_keepalive_reuses_the_client_connection_and_cleans_each_spill() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for path in ["/first", "/second"] {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let head = read_request_head(&mut stream).await;
            assert!(
                head.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                "{head}"
            );
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
            let chunk = vec![b'x'; SLOW_CHUNK_SIZE];
            for _ in 0..SLOW_BODY_SIZE / SLOW_CHUNK_SIZE {
                stream.write_all(&chunk).await.unwrap();
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            stream.flush().await.unwrap();
            let mut byte = [0_u8; 1];
            let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
                .await
                .expect("upstream should close after each complete response")
                .unwrap();
            assert_eq!(closed, 0);
        }
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp",
            &format!("server {upstream_address};"),
            1000,
            "150ms",
            "",
        ),
        &[],
    );
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);
    let temp_root = prefix.join("proxy-temp");

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(b"GET /first HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .unwrap();
    wait_for_generated_temp_file(&temp_root, true).await;
    let first = read_response_with_body(&mut client, SLOW_BODY_SIZE)
        .await
        .expect("first slow response should complete");
    assert_complete_response(&first);
    wait_for_generated_temp_file(&temp_root, false).await;

    client
        .write_all(b"GET /second HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    wait_for_generated_temp_file(&temp_root, true).await;
    let second = read_response_with_body(&mut client, SLOW_BODY_SIZE)
        .await
        .expect("second response should reuse the client connection");
    assert_complete_response(&second);
    wait_for_generated_temp_file(&temp_root, false).await;

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("Connection: close should stop the reused client connection")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("backend should serve exactly two slow responses")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn slow_upstream_pool_reuses_the_backend_connection_between_clients() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        for path in ["/pool-first", "/pool-second"] {
            let head = read_request_head(&mut stream).await;
            assert!(
                head.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                "{head}"
            );
            write_slow_response(&mut stream, "keep-alive").await;
        }
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp",
            &format!("server {upstream_address}; keepalive 2;"),
            1000,
            "150ms",
            "",
        ),
        &[],
    );
    let state = test_state(&config);
    let temp_root = prefix.join("proxy-temp");

    let first_frontend = bind_frontend().await;
    let first_address = first_frontend.local_addr().unwrap();
    let first_server = spawn_frontend(first_frontend, &config, state.clone());
    let first = request_slow_response(first_address, "/pool-first").await;
    assert_complete_response(&first);
    wait_for_generated_temp_file(&temp_root, false).await;
    tokio::time::timeout(Duration::from_secs(2), first_server)
        .await
        .expect("first client handler should stop after Connection: close")
        .unwrap();
    wait_for_pooled_connection(&state).await;

    let second_frontend = bind_frontend().await;
    let second_address = second_frontend.local_addr().unwrap();
    let second_server = spawn_frontend(second_frontend, &config, state.clone());
    let second = request_slow_response(second_address, "/pool-second").await;
    assert_complete_response(&second);
    wait_for_generated_temp_file(&temp_root, false).await;
    tokio::time::timeout(Duration::from_secs(2), second_server)
        .await
        .expect("second client handler should stop after Connection: close")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("the backend should serve both requests on one pooled connection")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn request_slow_response(frontend: SocketAddr, path: &str) -> Vec<u8> {
    let mut client = TcpStream::connect(frontend).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    read_response_with_body(&mut client, SLOW_BODY_SIZE)
        .await
        .expect("slow pooled response should complete")
}

async fn write_slow_response(stream: &mut TcpStream, connection: &str) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {SLOW_BODY_SIZE}\r\nConnection: {connection}\r\n\r\n"
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

async fn wait_for_pooled_connection(state: &SharedState) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.upstream_keepalive.len("p0_backend") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("complete slow response should return its upstream connection to the pool");
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
            eprintln!("skipping slow-upstream keepalive test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind slow-upstream keepalive backend: {error}"),
    }
}
