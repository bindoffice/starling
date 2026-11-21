use super::support::*;
use super::*;

#[tokio::test]
async fn applies_lingering_close_modes_to_connection_shutdown() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18569;
                    lingering_close off;
                    location /always {
                        lingering_close always;
                        lingering_time 200ms;
                        lingering_timeout 30ms;
                        return 200 "always\n";
                    }
                    location /on {
                        lingering_close on;
                        lingering_time 200ms;
                        lingering_timeout 30ms;
                        return 200 "on\n";
                    }
                    location /off {
                        lingering_close off;
                        return 200 "off\n";
                    }
                }
            }
        "#,
        &[],
    );

    let (always_client, always_task) = start_connection(
        &config,
        b"GET /always HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !always_task.is_finished(),
        "lingering_close always must wait even without pending input"
    );
    drop(always_client);
    tokio::time::timeout(Duration::from_secs(1), always_task)
        .await
        .unwrap()
        .unwrap();

    let request = b"GET /on HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n\
        GET /off HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let (on_client, on_task) = start_connection(&config, request).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        !on_task.is_finished(),
        "lingering_close on must drain pipelined input"
    );
    drop(on_client);
    tokio::time::timeout(Duration::from_secs(1), on_task)
        .await
        .unwrap()
        .unwrap();

    let request = b"GET /off HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n\
        GET /always HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let (off_client, off_task) = start_connection(&config, request).await;
    tokio::time::timeout(Duration::from_millis(100), off_task)
        .await
        .expect("lingering_close off must not drain pending input")
        .unwrap();
    drop(off_client);

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn resets_client_connections_after_request_timeout() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                client_header_timeout 40ms;
                server {
                    listen 127.0.0.1:18570;
                    reset_timedout_connection on;
                    lingering_close off;
                }
            }
        "#,
        &[],
    );
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping timeout reset test: binding a TCP listener is not permitted");
            let _ = std::fs::remove_dir_all(prefix);
            return;
        }
        Err(error) => panic!("bind timeout reset listener: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server = tokio::spawn(async move {
        let (stream, remote) = listener.accept().await.unwrap();
        let reset_target = TimeoutReset::from_tcp(&stream);
        handle_http_connection_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), ListenAddress::Inet(address)),
            group,
            state,
            reset_target,
            None,
        )
        .await;
    });

    let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    let error = client.read_to_end(&mut response).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionReset);
    assert!(
        response.starts_with(b"HTTP/1.1 408 Request Time-out\r\n"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    server.await.unwrap();

    let _ = std::fs::remove_dir_all(prefix);
}

async fn start_connection(
    config: &Arc<RuntimeConfig>,
    request: &[u8],
) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<()>) {
    let group = build_listener_groups(config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
    let state = test_state(config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client.write_all(request).await.unwrap();
    let mut response = [0_u8; 4096];
    let read = client.read(&mut response).await.unwrap();
    assert!(
        response[..read].starts_with(b"HTTP/1.1 200 OK\r\n"),
        "{}",
        String::from_utf8_lossy(&response[..read])
    );
    (client, task)
}
