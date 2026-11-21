use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_proxy_buffer_size() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_buffer_size 32k;
                server {
                    listen 127.0.0.1:13400;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13401;
                    proxy_pass 127.0.0.1:15460;
                    proxy_buffer_size 8k;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_buffer_size, 32 * 1024);
    assert_eq!(servers[1].proxy_buffer_size, 8 * 1024);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13402;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
        &[],
    );
    assert_eq!(
        config.stream.as_ref().unwrap().servers[0].proxy_buffer_size,
        16 * 1024
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_zero_stream_proxy_buffer_size() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13403;
                    proxy_pass 127.0.0.1:15460;
                    proxy_buffer_size 0;
                }
            }
        "#,
    );
    assert!(error.contains("proxy_buffer_size"));
    assert!(error.contains("nonzero"));
}

#[test]
fn accepts_deprecated_stream_buffer_aliases() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_downstream_buffer 24k;
                server {
                    listen 127.0.0.1:13404;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13405;
                    proxy_pass 127.0.0.1:15460;
                    proxy_upstream_buffer 12k;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_buffer_size, 24 * 1024);
    assert_eq!(servers[1].proxy_buffer_size, 12 * 1024);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn truncates_udp_datagrams_to_stream_proxy_buffer_size() {
    let Some((listener, client, upstream, listen_address, client_address)) =
        udp_test_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 64];
        let (size, source) =
            tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&buffer[..size], b"abcd");
        upstream.send_to(b"12345678", source).await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_buffer_size 4;
                    proxy_requests 1;
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

    sender.send(b"abcdefgh".to_vec()).await.unwrap();
    let mut response = [0_u8; 64];
    let (size, _) = tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response[..size], b"1234");

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
            eprintln!("skipping stream proxy buffer test: sockets not permitted");
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
