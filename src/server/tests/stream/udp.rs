use super::*;

#[test]
fn compiles_udp_stream_proxy_configuration() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    proxy_ssl on;
                    upstream dns_upstream {
                        server 127.0.0.1:15353;
                    }
                    server {
                        listen 127.0.0.1:15353 udp;
                        proxy_pass dns_upstream;
                        proxy_timeout 5s;
                        proxy_connect_timeout 2s;
                    }
                }
            "#,
        &[],
    );
    let stream = config.stream.as_ref().unwrap();
    assert!(stream.servers[0].listens[0].udp);
    assert!(stream.servers[0].proxy_ssl);
    assert_eq!(
        stream.servers[0].proxy_connect_timeout,
        Duration::from_secs(2)
    );
    let routes = build_stream_routes(&config);
    assert_eq!(routes.len(), 1);
    assert!(routes[0].udp);
    assert_eq!(routes[0].address.port(), 15353);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_plaintext_udp_sessions_when_proxy_ssl_is_enabled() {
    let upstream = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(socket) => socket,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP session test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind UDP upstream: {err}"),
    };
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(socket) => socket,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP session test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind UDP listener: {err}"),
    };
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_address = listener.local_addr().unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let client_address = client.local_addr().unwrap();
    let source = format!(
        r#"
                events {{}}
                stream {{
                    proxy_ssl on;
                    log_format datagrams
                        '$remote_addr $protocol $status $bytes_received $bytes_sent '
                        '$upstream_addr $upstream_bytes_sent $upstream_bytes_received';
                    server {{
                        listen {listen_address} udp;
                        access_log logs/udp-access.log datagrams;
                        proxy_pass {upstream_address};
                        proxy_timeout 5s;
                        proxy_connect_timeout 1s;
                    }}
                }}
            "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let state = test_state(&config);
    let listener = Arc::new(listener);
    let (sender, receiver) = mpsc::channel(4);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        listener.clone(),
        receiver,
        client_address,
        StreamRoute {
            address: ListenAddress::Inet(listen_address),
            ssl: false,
            udp: true,
            proxy_protocol: false,
            socket_options: ListenSocketOptions::default(),
        },
        state,
        shutdown,
    ));
    let upstream_task = tokio::spawn(async move {
        let mut buffer = [0_u8; 64];
        let (size, first_source) = upstream.recv_from(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..size], b"one");
        upstream.send_to(b"reply-one", first_source).await.unwrap();
        let (size, second_source) = upstream.recv_from(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..size], b"two");
        assert_eq!(first_source, second_source);
        upstream.send_to(b"reply-two", second_source).await.unwrap();
        first_source
    });

    sender.send(b"one".to_vec()).await.unwrap();
    let mut response = [0_u8; 64];
    let (size, from) =
        tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, listen_address);
    assert_eq!(&response[..size], b"reply-one");

    sender.send(b"two".to_vec()).await.unwrap();
    let (size, from) =
        tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, listen_address);
    assert_eq!(&response[..size], b"reply-two");
    upstream_task.await.unwrap();

    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), session)
        .await
        .unwrap()
        .unwrap();
    let logged = std::fs::read_to_string(prefix.join("logs/udp-access.log")).unwrap();
    assert_eq!(
        logged,
        format!("127.0.0.1 UDP 200 6 18 {upstream_address} 6 18\n")
    );
    let _ = std::fs::remove_dir_all(prefix);
}
