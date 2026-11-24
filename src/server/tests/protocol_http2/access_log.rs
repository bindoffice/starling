use super::*;

#[tokio::test]
async fn deferred_stream_access_log_is_written_after_the_h2_body_completes() {
    let upstream = match bind_test_tcp("deferred access log upstream").await {
        Some(listener) => listener,
        None => return,
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  6\r\nfirst\n\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\n\r\n")
            .await
            .unwrap();
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    log_format stream '$status|$upstream_response_time';
                    server {{
                        listen 127.0.0.1:18234 http2;
                        access_log logs/stream.log stream;
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45734".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(async move {
        connection.await.unwrap();
    });
    let request = http::Request::builder()
        .method("GET")
        .uri("http://localhost/stream")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    let first = tokio::time::timeout(Duration::from_secs(2), body.data())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    body.flow_control().release_capacity(first.len()).unwrap();
    assert_eq!(&first[..], b"first\n");

    tokio::time::sleep(Duration::from_millis(100)).await;
    let log_path = prefix.join("logs/stream.log");
    let before = std::fs::read_to_string(&log_path).unwrap_or_default();
    assert!(
        before.is_empty(),
        "access log was written before the streamed body completed: {before}"
    );

    release.send(()).unwrap();
    let mut rest = Vec::new();
    while let Some(data) = tokio::time::timeout(Duration::from_secs(2), body.data())
        .await
        .unwrap()
    {
        let data = data.unwrap();
        body.flow_control().release_capacity(data.len()).unwrap();
        rest.extend_from_slice(&data);
    }
    assert_eq!(rest, b"second\n");

    let mut log = String::new();
    for _ in 0..50 {
        log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if !log.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (status, response_time) = log.trim_end().split_once('|').unwrap();
    assert_eq!(status, "200");
    assert!(response_time.parse::<f64>().unwrap() >= 0.1, "{log}");

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .unwrap()
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
