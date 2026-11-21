use crate::server::stream::{StreamProxySettings, relay_stream};

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_proxy_rates() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_upload_rate 8k;
                proxy_download_rate $server_port;
                server {
                    listen 127.0.0.1:13360;
                    proxy_pass 127.0.0.1:15460;
                    proxy_upload_rate 2k;
                }
                server {
                    listen 127.0.0.1:13361;
                    proxy_pass 127.0.0.1:15460;
                    proxy_download_rate 4m;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_upload_rate.as_deref(), Some("2k"));
    assert_eq!(
        servers[0].proxy_download_rate.as_deref(),
        Some("$server_port")
    );
    assert_eq!(servers[1].proxy_upload_rate.as_deref(), Some("8k"));
    assert_eq!(servers[1].proxy_download_rate.as_deref(), Some("4m"));
    let _ = std::fs::remove_dir_all(prefix);

    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13362;
                    proxy_pass 127.0.0.1:15460;
                    proxy_upload_rate invalid;
                }
            }
        "#,
    );
    assert!(error.contains("proxy_upload_rate"));
    assert!(error.contains("invalid value"));
}

#[tokio::test]
async fn throttles_tcp_stream_directions() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13363;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
        &[],
    );
    let state = test_state(&config);
    let remote = ListenAddress::Inet("127.0.0.1:50000".parse().unwrap());
    let payload = vec![b'x'; 16 * 1024];

    let (mut client, inbound) = tokio::io::duplex(64 * 1024);
    let (outbound, mut backend) = tokio::io::duplex(64 * 1024);
    let state = state.clone();
    let relay_remote = remote.clone();
    let relay = tokio::spawn(async move {
        let mut inbound = inbound;
        let mut outbound = outbound;
        relay_stream(
            &mut inbound,
            &mut outbound,
            Duration::from_secs(5),
            StreamProxySettings {
                upload_rate: 100 * 1024,
                download_rate: 0,
                buffer_size: 16 * 1024,
                half_close: true,
            },
            &relay_remote,
            &state.logger,
        )
        .await
    });
    client.write_all(&payload).await.unwrap();
    client.shutdown().await.unwrap();
    let started = std::time::Instant::now();
    let mut received = Vec::new();
    backend.read_to_end(&mut received).await.unwrap();
    let upload_elapsed = started.elapsed();
    assert_eq!(received, payload);
    assert!(
        upload_elapsed >= Duration::from_millis(50),
        "TCP upload rate limit was not applied: {upload_elapsed:?}"
    );
    backend.shutdown().await.unwrap();
    relay.await.unwrap();

    let (mut client, inbound) = tokio::io::duplex(64 * 1024);
    let (outbound, mut backend) = tokio::io::duplex(64 * 1024);
    let state = test_state(&config);
    let remote = remote.clone();
    let relay = tokio::spawn(async move {
        let mut inbound = inbound;
        let mut outbound = outbound;
        relay_stream(
            &mut inbound,
            &mut outbound,
            Duration::from_secs(5),
            StreamProxySettings {
                upload_rate: 0,
                download_rate: 100 * 1024,
                buffer_size: 16 * 1024,
                half_close: true,
            },
            &remote,
            &state.logger,
        )
        .await
    });
    client.shutdown().await.unwrap();
    let received = tokio::spawn(async move {
        let started = std::time::Instant::now();
        let mut received = Vec::new();
        client.read_to_end(&mut received).await.unwrap();
        (received, started.elapsed())
    });
    backend.write_all(&payload).await.unwrap();
    backend.shutdown().await.unwrap();
    let (received, download_elapsed) = received.await.unwrap();
    assert_eq!(received, payload);
    assert!(
        download_elapsed >= Duration::from_millis(50),
        "TCP download rate limit was not applied: {download_elapsed:?}"
    );
    relay.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn throttles_udp_upload_datagrams() {
    let Some((listener, _client, upstream, listen_address, client_address)) = udp_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 256];
        let (first_size, _) =
            tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(first_size, 100);
        let started = std::time::Instant::now();
        let (second_size, _) =
            tokio::time::timeout(Duration::from_secs(1), upstream.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(second_size, 100);
        started.elapsed()
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    set $upload_limit 1k;
                    proxy_upload_rate $upload_limit;
                    proxy_requests 2;
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (sender, receiver) = mpsc::channel(8);
    let (shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));

    sender.send(vec![b'a'; 100]).await.unwrap();
    sender.send(vec![b'b'; 100]).await.unwrap();
    let elapsed = upstream_task.await.unwrap();
    assert!(
        elapsed >= Duration::from_millis(70),
        "UDP upload rate limit was not applied: {elapsed:?}"
    );
    shutdown_sender.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(1), session)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn throttles_udp_download_datagrams() {
    let Some((listener, client, upstream, listen_address, client_address)) = udp_sockets().await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 256];
        let (_, source) = upstream.recv_from(&mut buffer).await.unwrap();
        upstream.send_to(&[b'a'; 100], source).await.unwrap();
        upstream.send_to(&[b'b'; 100], source).await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_download_rate 1k;
                    proxy_requests 1;
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (sender, receiver) = mpsc::channel(8);
    let (shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));

    sender.send(b"ping".to_vec()).await.unwrap();
    let mut buffer = [0_u8; 256];
    tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let started = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(1), client.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(70),
        "UDP download rate limit was not applied: {elapsed:?}"
    );
    shutdown_sender.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(1), session)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn udp_sockets() -> Option<(UdpSocket, UdpSocket, UdpSocket, SocketAddr, SocketAddr)> {
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(socket) => socket,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream rate test: sockets not permitted");
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
