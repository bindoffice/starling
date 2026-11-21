use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_udp_packet_limits() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_requests 3;
                proxy_responses 2;
                server {
                    listen 127.0.0.1:13350 udp;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13351 udp;
                    proxy_requests 7;
                    proxy_responses 1;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_requests, 3);
    assert_eq!(servers[0].proxy_responses, 2);
    assert_eq!(servers[1].proxy_requests, 7);
    assert_eq!(servers[1].proxy_responses, 1);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13352 udp;
                    return defaults;
                }
            }
        "#,
        &[],
    );
    let server = &config.stream.as_ref().unwrap().servers[0];
    assert_eq!(server.proxy_requests, 0);
    assert_eq!(server.proxy_responses, STREAM_PROXY_RESPONSES_UNLIMITED);
    let _ = std::fs::remove_dir_all(prefix);

    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_responses -1;
                server {
                    listen 127.0.0.1:13353 udp;
                    return invalid;
                }
            }
        "#,
    );
    assert!(error.contains("proxy_responses"));
    assert!(error.contains("invalid value"));
}

#[tokio::test]
async fn closes_udp_session_after_request_and_response_limits() {
    let Some((listener, client, upstream, listen_address, client_address)) =
        udp_test_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 64];
        for (request, response) in [
            (&b"one"[..], &b"reply-one"[..]),
            (&b"two"[..], &b"reply-two"[..]),
        ] {
            let (size, source) =
                tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut buffer))
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(&buffer[..size], request);
            upstream.send_to(response, source).await.unwrap();
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), upstream.recv_from(&mut buffer))
                .await
                .is_err(),
            "UDP session forwarded more requests than proxy_requests allows"
        );
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_requests 2;
                    proxy_responses 1;
                    proxy_timeout 2s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (sender, receiver) = mpsc::channel(8);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));

    sender.send(b"one".to_vec()).await.unwrap();
    assert_udp_response(&client, b"reply-one").await;
    sender.send(b"two".to_vec()).await.unwrap();
    assert_udp_response(&client, b"reply-two").await;

    tokio::time::timeout(Duration::from_secs(1), session)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn closes_udp_session_after_finite_responses_without_request_limit() {
    let Some((listener, client, upstream, listen_address, client_address)) =
        udp_test_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 64];
        let (size, source) = upstream.recv_from(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..size], b"ping");
        upstream.send_to(b"pong", source).await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_responses 1;
                    proxy_timeout 2s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (sender, receiver) = mpsc::channel(8);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));

    sender.send(b"ping".to_vec()).await.unwrap();
    assert_udp_response(&client, b"pong").await;
    tokio::time::timeout(Duration::from_secs(1), session)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stops_reading_udp_requests_after_the_request_limit() {
    let Some((listener, _client, upstream, listen_address, client_address)) =
        udp_test_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 64];
        let (size, _) = upstream.recv_from(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..size], b"one");
        assert!(
            tokio::time::timeout(Duration::from_millis(120), upstream.recv_from(&mut buffer))
                .await
                .is_err(),
            "UDP session read another datagram after proxy_requests was reached"
        );
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_requests 1;
                    proxy_timeout 50ms;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (sender, receiver) = mpsc::channel(8);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));

    sender.send(b"one".to_vec()).await.unwrap();
    let _ = sender.send(b"two".to_vec()).await;
    tokio::time::timeout(Duration::from_secs(1), session)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn udp_test_sockets() -> Option<(UdpSocket, UdpSocket, UdpSocket, SocketAddr, SocketAddr)> {
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(socket) => socket,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream UDP packet limit test: sockets not permitted");
            return None;
        }
        Err(err) => panic!("bind UDP listener: {err}"),
    };
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_address = listener.local_addr().unwrap();
    let client_address = client.local_addr().unwrap();
    Some((listener, client, upstream, listen_address, client_address))
}

async fn assert_udp_response(client: &UdpSocket, expected: &[u8]) {
    let mut buffer = [0_u8; 64];
    let (size, _) = tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..size], expected);
}
