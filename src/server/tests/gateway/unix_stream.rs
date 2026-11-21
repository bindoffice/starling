use super::*;

#[cfg(unix)]
#[tokio::test]
async fn proxies_stream_traffic_through_a_unix_upstream() {
    let socket_path = test_socket_path("stream");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    upstream local_app {{
                        server unix:{};
                    }}
                    server {{
                        listen 127.0.0.1:18133;
                        proxy_pass local_app;
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(
            "127.0.0.1:45678".parse().unwrap(),
        ));
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    client.write_all(b"ping").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"pong");
    proxy.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn proxies_stream_traffic_from_a_unix_listen_socket() {
    let socket_path = test_socket_path("stream-listen");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            let _ = std::fs::remove_file(&socket_path);
            eprintln!("skipping UNIX stream listen test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind stream backend: {error}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen unix:{};
                        proxy_pass {backend_address};
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    assert_eq!(route.address.unix_path(), Some(socket_path.as_path()));
    let state = test_state(&config);
    let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let task = tokio::spawn(stream_unix_accept_loop(listener, route, state, shutdown_rx));

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client.write_all(b"ping").await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"pong");

    shutdown_tx.send(ShutdownMode::Fast).unwrap();
    task.await.unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}
