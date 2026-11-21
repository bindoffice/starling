use super::*;

#[cfg(unix)]
#[tokio::test]
async fn proxies_http_traffic_over_a_unix_socket() {
    let socket_path = test_socket_path("http-proxy");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await.unwrap();
        assert_eq!(request_line, "GET /hello HTTP/1.1\r\n");
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        reader
                .get_mut()
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\nConnection: close\r\n\r\nunix-proxy",
                )
                .await
                .unwrap();
        reader.get_mut().shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18134;
                        location / {{
                            proxy_pass http://unix:{};
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("unix-proxy"), "{response}");
    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn formats_unix_listen_addresses_for_http_variables() {
    let socket_path = test_socket_path("http-variables");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen unix:{};
                        location / {{
                            return 200 "$server_addr|$remote_addr|$server_port|$remote_port\n";
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote = ListenAddress::Unix(PathBuf::new());
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        handle_io(
            server,
            ConnectionAddresses::direct(remote, local),
            group,
            state,
        )
        .await;
    });
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.ends_with(&format!("unix:{}|unix:||\n", socket_path.display())),
        "{response}"
    );
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn serves_http_on_a_unix_listen_socket() {
    let socket_path = test_socket_path("http-listen");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen unix:{};
                        location / {{
                            return 200 "$server_addr|$remote_addr|$server_port|$remote_port\n";
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    assert_eq!(group.address.unix_path(), Some(socket_path.as_path()));
    let state = test_state(&config);
    let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let task = tokio::spawn(accept_unix_loop(listener, group, state, shutdown_rx));

    let mut client = UnixStream::connect(&socket_path).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.ends_with(&format!("unix:{}|unix:||\n", socket_path.display())),
        "{response}"
    );

    shutdown_tx.send(ShutdownMode::Fast).unwrap();
    task.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}
