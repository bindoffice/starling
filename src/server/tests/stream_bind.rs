use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_proxy_bind() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_bind 127.0.0.1;
                server {
                    listen 127.0.0.1:13420;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13421;
                    proxy_pass 127.0.0.1:15460;
                    proxy_bind off;
                }
                server {
                    listen 127.0.0.1:13422;
                    proxy_pass 127.0.0.1:15460;
                    proxy_bind $remote_addr;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_bind.as_deref(), Some("127.0.0.1"));
    assert_eq!(servers[1].proxy_bind, None);
    assert_eq!(servers[2].proxy_bind.as_deref(), Some("$remote_addr"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_unsupported_stream_proxy_bind_options() {
    let transparent = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13423;
                    proxy_pass 127.0.0.1:15460;
                    proxy_bind 127.0.0.1 transparent;
                }
            }
        "#,
    );
    assert!(transparent.contains("proxy_bind"));
    assert!(transparent.contains("transparent"));

    let parameters = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13424;
                    proxy_pass 127.0.0.1:15460;
                    proxy_bind 127.0.0.1 invalid;
                }
            }
        "#,
    );
    assert!(parameters.contains("invalid parameter \"invalid\""));
}

#[tokio::test]
async fn binds_tcp_upstream_to_configured_local_address() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream proxy_bind test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind stream backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, peer) = backend.accept().await.unwrap();
        assert_eq!(peer.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:13425;
                    proxy_pass {backend_address};
                    proxy_bind 127.0.0.2;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "127.0.0.1:45680".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });

    client.write_all(b"ping").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"pong");
    proxy.await.unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn binds_udp_upstream_to_configured_local_address() {
    let Some((listener, client, upstream, listen_address, client_address)) =
        udp_test_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut request = [0_u8; 32];
        let (size, source) =
            tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut request))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(&request[..size], b"ping");
        assert_eq!(source.ip(), "127.0.0.2".parse::<IpAddr>().unwrap());
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
                    proxy_bind 127.0.0.2;
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

    sender.send(b"ping".to_vec()).await.unwrap();
    let mut response = [0_u8; 32];
    let (size, _) = tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response[..size], b"pong");
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
            eprintln!("skipping stream proxy_bind test: sockets not permitted");
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
