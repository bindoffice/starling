use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn client_disconnect_cancels_an_inflight_attempt_without_starting_retry() {
    let first = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy retry cancellation test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind first upstream: {error}"),
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let frontend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        assert_eq!(
            read_request_body(&mut stream).await,
            b"partmore",
            "first attempt did not receive the complete file-backed body"
        );
        request_seen.send(()).unwrap();
        release_rx.await.unwrap();
        let _ = stream
            .write_all(b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await;
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("client cancellation still started a second upstream attempt");
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18582;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server_task = tokio::spawn(async move {
        let (stream, remote) = frontend_listener.accept().await.unwrap();
        let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
        let local = stream
            .local_addr()
            .map(ListenAddress::Inet)
            .unwrap_or_else(|_| group.address.clone());
        handle_io_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
            None,
            control,
        )
        .await;
    });

    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npartmore",
        )
        .await
        .unwrap();
    request_seen_rx.await.unwrap();
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();

    first_backend.await.unwrap();
    second_backend.await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP connection should stop after the client disconnects")
        .unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "cancelled retry attempt leaked its file-backed request body"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn unix_client_close_cancels_an_inflight_attempt_without_starting_retry() {
    let first_path = test_socket_path("proxy-retry-cancel-unix-first");
    let second_path = test_socket_path("proxy-retry-cancel-unix-second");
    let frontend_path = test_socket_path("proxy-retry-cancel-unix-frontend");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let Some(frontend_listener) = test_unix_listener(&frontend_path) else {
        return;
    };
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        assert_eq!(
            read_request_body(&mut stream).await,
            b"partmore",
            "first attempt did not receive the complete file-backed body"
        );
        request_seen.send(()).unwrap();
        release_rx.await.unwrap();
        let _ = stream
            .write_all(
                b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\
                  Connection: close\r\n\r\n",
            )
            .await;
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("unix client cancellation still started a second upstream attempt");
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server unix:{};
                        server unix:{};
                    }}
                    server {{
                        listen unix:{};
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
            first_path.display(),
            second_path.display(),
            frontend_path.display(),
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(accept_unix_loop(
        frontend_listener,
        group,
        state,
        shutdown_rx,
    ));

    let mut client = UnixStream::connect(&frontend_path).await.unwrap();
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npartmore",
        )
        .await
        .unwrap();
    request_seen_rx.await.unwrap();
    drop(client);
    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();

    first_backend.await.unwrap();
    second_backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "cancelled Unix client retry attempt leaked its file-backed request body"
    );
    shutdown.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Unix accept loop should stop")
        .unwrap();

    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_file(frontend_path);
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_body<R>(stream: &mut R) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    };
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before the complete request body");
        request.extend_from_slice(&buffer[..read]);
    }
    request[header_end..header_end + content_length].to_vec()
}

async fn wait_for_temp_cleanup(path: &std::path::Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
