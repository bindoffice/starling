use super::*;

#[tokio::test]
async fn client_abort_drops_an_incomplete_upstream_keepalive_connection() {
    let Some(upstream) = bind_listener("keepalive cancellation upstream").await else {
        return;
    };
    let Some(frontend_listener) = bind_listener("keepalive cancellation frontend").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let frontend_address = frontend_listener.local_addr().unwrap();
    let (first_closed, first_closed_rx) = tokio::sync::oneshot::channel();

    let backend = tokio::spawn(async move {
        let (first, _) = upstream.accept().await.unwrap();
        let mut first = BufReader::new(first);
        let head = read_request_head(&mut first).await;
        assert!(head.starts_with("GET /first HTTP/1.1\r\n"), "{head}");
        first
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: keep-alive\r\n\r\nfirst",
            )
            .await
            .unwrap();

        let mut byte = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), first.read(&mut byte))
            .await
            .expect("cancelled upstream connection should be closed")
            .unwrap();
        assert_eq!(read, 0, "cancelled upstream connection still had data");
        first_closed.send(()).unwrap();

        let (second, _) = tokio::time::timeout(Duration::from_secs(2), upstream.accept())
            .await
            .expect("next request should open a fresh upstream connection")
            .unwrap();
        let mut second = BufReader::new(second);
        let head = read_request_head(&mut second).await;
        assert!(head.starts_with("GET /second HTTP/1.1\r\n"), "{head}");
        second
            .get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecond")
            .await
            .unwrap();
    });

    let (prefix, config) = keepalive_config(upstream_address, frontend_address, "");
    let state = test_state(&config);
    let frontend = spawn_frontend(frontend_listener, &config, state.clone(), 2);

    let mut first = TcpStream::connect(frontend_address).await.unwrap();
    first
        .write_all(b"GET /first HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    read_until(&mut first, b"first").await;
    socket2::SockRef::from(&first)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(first);
    first_closed_rx.await.unwrap();

    assert_eq!(
        state.upstream_keepalive.len("backend"),
        0,
        "an incomplete response connection must not enter the keepalive pool"
    );

    let mut second = TcpStream::connect(frontend_address).await.unwrap();
    second
        .write_all(b"GET /second HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    second.read_to_end(&mut response).await.unwrap();
    assert!(
        response.windows(6).any(|window| window == b"second"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    backend.await.unwrap();
    frontend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn ignore_client_abort_drains_and_reuses_the_upstream_connection() {
    let Some(upstream) = bind_listener("keepalive ignore-abort upstream").await else {
        return;
    };
    let Some(frontend_listener) = bind_listener("keepalive ignore-abort frontend").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let frontend_address = frontend_listener.local_addr().unwrap();
    let (release, release_rx) = tokio::sync::oneshot::channel();

    let backend = tokio::spawn(async move {
        let (connection, _) = upstream.accept().await.unwrap();
        let mut connection = BufReader::new(connection);
        let head = read_request_head(&mut connection).await;
        assert!(head.starts_with("GET /first HTTP/1.1\r\n"), "{head}");
        connection
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: keep-alive\r\n\r\nfirst",
            )
            .await
            .unwrap();
        release_rx.await.unwrap();
        connection.get_mut().write_all(b"second").await.unwrap();

        let head = tokio::time::timeout(Duration::from_secs(2), read_request_head(&mut connection))
            .await
            .expect("preserved connection should accept the next request");
        assert!(head.starts_with("GET /reuse HTTP/1.1\r\n"), "{head}");
        connection
            .get_mut()
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nreused")
            .await
            .unwrap();

        if tokio::time::timeout(Duration::from_millis(250), upstream.accept())
            .await
            .is_ok()
        {
            panic!("preserved request opened a second upstream connection");
        }
    });

    let (prefix, config) = keepalive_config(
        upstream_address,
        frontend_address,
        "proxy_ignore_client_abort on;",
    );
    let state = test_state(&config);
    let frontend = spawn_frontend(frontend_listener, &config, state.clone(), 2);

    let mut first = TcpStream::connect(frontend_address).await.unwrap();
    first
        .write_all(b"GET /first HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    read_until(&mut first, b"first").await;
    socket2::SockRef::from(&first)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(first);

    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.upstream_keepalive.len("backend") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("drained response should return its connection to the keepalive pool");

    let mut second = TcpStream::connect(frontend_address).await.unwrap();
    second
        .write_all(b"GET /reuse HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    second.read_to_end(&mut response).await.unwrap();
    assert!(
        response.windows(6).any(|window| window == b"reused"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    backend.await.unwrap();
    frontend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn client_abort_keeps_a_multiplexed_http2_upstream_connection_usable() {
    let Some(upstream) = bind_listener("HTTP/2 keepalive cancellation upstream").await else {
        return;
    };
    let Some(frontend_listener) = bind_listener("HTTP/2 keepalive cancellation frontend").await
    else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let frontend_address = frontend_listener.local_addr().unwrap();

    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/first");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(())
            .unwrap();
        let mut first = respond.send_response(response, false).unwrap();
        first
            .send_data(Bytes::from_static(b"first"), false)
            .unwrap();

        let (request, mut respond) = tokio::select! {
            accepted = connection.accept() => accepted
                .expect("HTTP/2 upstream connection closed before the second stream")
                .expect("HTTP/2 upstream stream"),
            accepted = upstream.accept() => {
                let _ = accepted;
                panic!("HTTP/2 cancellation should keep the existing upstream connection");
            }
        };
        assert_eq!(request.uri().path(), "/reuse");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(())
            .unwrap();
        let mut second = respond.send_response(response, false).unwrap();
        second
            .send_data(Bytes::from_static(b"reused"), true)
            .unwrap();

        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 2;
                    }}
                    server {{
                        listen {frontend_address};
                        location / {{
                            proxy_pass http://backend;
                            proxy_http_version 2;
                            proxy_buffering off;
                            proxy_read_timeout 2s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let frontend = spawn_frontend(frontend_listener, &config, state, 2);

    let mut first = TcpStream::connect(frontend_address).await.unwrap();
    first
        .write_all(b"GET /first HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    read_until(&mut first, b"first").await;
    socket2::SockRef::from(&first)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(first);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut second = TcpStream::connect(frontend_address).await.unwrap();
    second
        .write_all(b"GET /reuse HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    second.read_to_end(&mut response).await.unwrap();
    assert!(
        response.windows(6).any(|window| window == b"reused"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    backend.await.unwrap();
    frontend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label} test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

fn keepalive_config(
    upstream_address: SocketAddr,
    frontend_address: SocketAddr,
    proxy_options: &str,
) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 2;
                    }}
                    server {{
                        listen {frontend_address};
                        location / {{
                            proxy_pass http://backend;
                            proxy_http_version 1.1;
                            proxy_buffering off;
                            proxy_read_timeout 2s;
                            {proxy_options}
                        }}
                    }}
                }}
            "#
        ),
        &[],
    )
}

fn spawn_frontend(
    listener: TcpListener,
    config: &Arc<RuntimeConfig>,
    state: SharedState,
    connections: usize,
) -> tokio::task::JoinHandle<()> {
    let group = build_listener_groups(config)[0].clone();
    tokio::spawn(async move {
        let mut handlers = Vec::with_capacity(connections);
        for _ in 0..connections {
            let (stream, remote) = listener.accept().await.unwrap();
            let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
            let group = group.clone();
            let state = state.clone();
            handlers.push(tokio::spawn(async move {
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
            }));
        }
        for handler in handlers {
            handler.await.unwrap();
        }
    })
}

async fn read_request_head<R>(reader: &mut R) -> String
where
    R: AsyncBufRead + Unpin,
{
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        let complete = line == "\r\n";
        head.push_str(&line);
        if complete {
            return head;
        }
    }
}

async fn read_until<R>(reader: &mut R, needle: &[u8])
where
    R: AsyncRead + Unpin,
{
    let mut received = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut buffer))
            .await
            .expect("timed out waiting for upstream response")
            .unwrap();
        assert!(read > 0, "connection closed before {needle:?}");
        received.extend_from_slice(&buffer[..read]);
        if received
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return;
        }
    }
}
