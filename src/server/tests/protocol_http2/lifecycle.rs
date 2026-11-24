use super::super::support::*;
use super::super::*;

async fn read_h2_body_without_window_updates(mut body: h2::RecvStream) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(data) = body.data().await {
        output.extend_from_slice(&data.unwrap());
    }
    output
}

#[tokio::test]
async fn enforces_keepalive_requests_on_http2_connections() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18462 http2;
                keepalive_requests 2;
                keepalive_timeout 5s;
                location / { return 200 "ok\n"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45684".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let mut builder = h2::client::Builder::new();
    builder.initial_window_size(1 << 20);
    let (mut sender, mut connection) = builder.handshake::<_, Bytes>(client).await.unwrap();
    connection.set_target_window_size(1 << 21);
    let driver = tokio::spawn(connection);

    for _ in 0..2 {
        let request = http::Request::builder()
            .uri("http://localhost/")
            .body(())
            .unwrap();
        let (response, _) = sender.send_request(request, true).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            super::read_h2_response_body(response.into_body()).await,
            b"ok\n"
        );
    }

    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let denied = match sender.send_request(request, true) {
        Err(_) => true,
        Ok((response, _)) => tokio::time::timeout(Duration::from_secs(1), response)
            .await
            .map(|response| response.is_err())
            .unwrap_or(true),
    };
    assert!(denied, "third request should be rejected after GOAWAY");

    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should close after the request limit")
        .unwrap();
    driver.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn drains_large_responses_before_keepalive_goaway() {
    let body = "SEE-THAT".repeat(50_000);
    let static_path = std::env::temp_dir().join(format!(
        "starling-h2-large-body-{}.html",
        std::process::id()
    ));
    std::fs::write(&static_path, &body).unwrap();
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18468 http2;
                keepalive_requests 2;
                keepalive_timeout 5s;
                location / { alias @FILE@; }
            }
        }
    "#
    .replace("@FILE@", &static_path.display().to_string());
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45688".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let mut builder = h2::client::Builder::new();
    builder.initial_window_size(1 << 20);
    let (mut sender, mut connection) = builder.handshake::<_, Bytes>(client).await.unwrap();
    connection.set_target_window_size(1 << 21);
    let driver = tokio::spawn(connection);

    for request_number in 1..=2 {
        let request = http::Request::builder()
            .uri("http://localhost/")
            .body(())
            .unwrap();
        let (response, _) = sender.send_request(request, true).unwrap();
        let response = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(
            Duration::from_secs(5),
            read_h2_body_without_window_updates(response.into_body()),
        )
        .await
        .unwrap_or_else(|_| panic!("response {request_number} body timed out"));
        assert_eq!(
            body.len(),
            400_000,
            "response {request_number} was truncated"
        );
    }

    drop(sender);
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("HTTP/2 server should close after draining the second response")
        .unwrap();
    driver.abort();
    let _ = std::fs::remove_file(static_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn closes_idle_http2_connections_after_keepalive_timeout() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18463 http2;
                keepalive_timeout 100ms;
                location / { return 200 "ok\n"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45685".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);

    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        super::read_h2_response_body(response.into_body()).await,
        b"ok\n"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("idle HTTP/2 connection should time out")
        .unwrap();
    driver.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn graceful_shutdown_sends_goaway_and_rejects_new_streams() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18464 http2;
                keepalive_timeout 5s;
                location / { return 200 "ok\n"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(handle_http_connection_with_shutdown(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
        None,
        None,
        Some(shutdown_rx),
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);

    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        super::read_h2_response_body(response.into_body()).await,
        b"ok\n"
    );

    shutdown.send(ShutdownMode::Graceful).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("graceful HTTP/2 shutdown should finish after active streams drain")
        .unwrap();

    let request = http::Request::builder()
        .uri("http://localhost/after-goaway")
        .body(())
        .unwrap();
    let rejected = match sender.send_request(request, true) {
        Err(_) => true,
        Ok((response, _)) => tokio::time::timeout(Duration::from_secs(1), response)
            .await
            .map(|response| response.is_err())
            .unwrap_or(true),
    };
    assert!(
        rejected,
        "new HTTP/2 streams must be rejected after graceful GOAWAY"
    );

    drop(sender);
    driver.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn graceful_shutdown_drains_an_in_flight_http2_stream() {
    let Some(upstream) = super::bind_test_tcp("graceful HTTP/2 drain").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let head = read_request_head(&mut stream).await;
        assert!(head.starts_with("GET /stream HTTP/1.1\r\n"), "{head}");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\
                  Content-Type: text/plain\r\nConnection: close\r\n\r\n\
                  5\r\nfirst\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n").await.unwrap();
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18465 http2;
                    keepalive_timeout 5s;
                    location / {{
                        proxy_pass http://{upstream_address};
                        proxy_buffering off;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45687".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(handle_http_connection_with_shutdown(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
        None,
        None,
        Some(shutdown_rx),
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);

    let request = http::Request::builder()
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
    let first = body.data().await.unwrap().unwrap();
    body.flow_control().release_capacity(first.len()).unwrap();
    assert_eq!(&first[..], b"first");

    shutdown.send(ShutdownMode::Graceful).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !server_task.is_finished(),
        "graceful shutdown must wait for the in-flight stream"
    );
    release.send(()).unwrap();

    let rest = tokio::time::timeout(Duration::from_secs(2), super::read_h2_response_body(body))
        .await
        .expect("in-flight HTTP/2 response should finish during graceful shutdown");
    assert_eq!(rest, b"second");
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should exit after the in-flight stream drains")
        .unwrap();
    backend.await.unwrap();

    drop(sender);
    driver.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_head(stream: &mut TcpStream) -> String {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).await.unwrap();
        head.push(byte[0]);
    }
    String::from_utf8(head).unwrap()
}
