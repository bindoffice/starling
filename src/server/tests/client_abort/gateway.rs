use super::support::*;
use super::*;

#[derive(Clone, Copy)]
enum RawGateway {
    Uwsgi,
    Scgi,
}

impl RawGateway {
    fn prefix(self) -> &'static str {
        match self {
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }

    fn config(self, upstream: SocketAddr, port: u16, preserve: bool) -> String {
        let prefix = self.prefix();
        let ignore_abort = if preserve {
            format!("{prefix}_ignore_client_abort on;")
        } else {
            String::new()
        };
        format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {prefix}_pass {upstream};
                            {prefix}_buffering off;
                            {ignore_abort}
                            {prefix}_read_timeout 30s;
                        }}
                    }}
                }}
            "#
        )
    }

    fn store_config(self, upstream: SocketAddr, port: u16) -> String {
        let prefix = self.prefix();
        format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {prefix}_pass {upstream};
                            {prefix}_buffering off;
                            {prefix}_store on;
                            {prefix}_store_access user:r group:r;
                            {prefix}_read_timeout 30s;
                        }}
                    }}
                }}
            "#
        )
    }

    fn cache_config(self, upstream: SocketAddr, port: u16) -> String {
        let prefix = self.prefix();
        format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    {prefix}_cache_path cache keys_zone=gateway_cache:1m;
                    server {{
                        listen 127.0.0.1:{port};
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            {prefix}_pass {upstream};
                            {prefix}_buffering off;
                            {prefix}_cache gateway_cache;
                            {prefix}_cache_key "$request_method$host$request_uri";
                            {prefix}_cache_valid 200 1m;
                            {prefix}_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        )
    }

    async fn read_request(self, stream: &mut TcpStream) {
        match self {
            Self::Uwsgi => read_uwsgi_header(stream).await,
            Self::Scgi => read_scgi_header(stream).await,
        }
    }
}

#[tokio::test]
async fn uwsgi_ignore_client_abort_keeps_waiting_for_response_headers() {
    assert_gateway_ignore_client_abort(RawGateway::Uwsgi, 18745).await;
}

#[tokio::test]
async fn scgi_ignore_client_abort_keeps_waiting_for_response_headers() {
    assert_gateway_ignore_client_abort(RawGateway::Scgi, 18746).await;
}

#[tokio::test]
async fn uwsgi_client_reset_cancels_upstream_while_waiting_for_response_headers() {
    assert_gateway_reset_cancels(RawGateway::Uwsgi, 18747).await;
}

#[tokio::test]
async fn scgi_client_reset_cancels_upstream_while_waiting_for_response_headers() {
    assert_gateway_reset_cancels(RawGateway::Scgi, 18748).await;
}

#[tokio::test]
async fn uwsgi_store_finishes_after_client_abort() {
    assert_gateway_store_survives_client_abort(RawGateway::Uwsgi, 18749).await;
}

#[tokio::test]
async fn scgi_store_finishes_after_client_abort() {
    assert_gateway_store_survives_client_abort(RawGateway::Scgi, 18750).await;
}

#[tokio::test]
async fn uwsgi_cache_finishes_after_client_abort() {
    assert_gateway_cache_survives_client_abort(RawGateway::Uwsgi, 18751).await;
}

#[tokio::test]
async fn scgi_cache_finishes_after_client_abort() {
    assert_gateway_cache_survives_client_abort(RawGateway::Scgi, 18752).await;
}

async fn assert_gateway_ignore_client_abort(gateway: RawGateway, port: u16) {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        gateway.read_request(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        release_rx.await.unwrap();

        let mut buffer = [0_u8; 1];
        let premature =
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer)).await;
        assert!(
            premature.is_err(),
            "{} ignore_client_abort cancelled the upstream while headers were pending: \
             {premature:?}",
            gateway.name()
        );
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 11\r\n\r\nfirstsecond",
            )
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("preserved gateway upstream should close after the complete response");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected {} upstream read after preserved response: {closed:?}",
            gateway.name()
        );
    });

    let (prefix, config) = test_config(&gateway.config(upstream, port, true), &[]);
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("gateway upstream should receive the request before the client reset")
        .unwrap();

    reset_client(client);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after draining the preserved response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn assert_gateway_reset_cancels(gateway: RawGateway, port: u16) {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        gateway.read_request(&mut stream).await;
        request_seen_tx.send(()).unwrap();

        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("client reset should cancel the gateway upstream while headers are pending");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected {} upstream read after client reset: {closed:?}",
            gateway.name()
        );
    });

    let (prefix, config) = test_config(&gateway.config(upstream, port, false), &[]);
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("gateway upstream should receive the request before the client reset")
        .unwrap();

    reset_client(client);

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after cancelling the gateway dispatch")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn assert_gateway_store_survives_client_abort(gateway: RawGateway, port: u16) {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        gateway.read_request(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 11\r\n\r\nfirst",
            )
            .await
            .unwrap();
        stream.flush().await.unwrap();
        release_rx.await.unwrap();
        stream.write_all(b"second").await.unwrap();
        stream.flush().await.unwrap();

        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("stored gateway upstream should close after the preserved tee completes");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected {} upstream read after stored response: {closed:?}",
            gateway.name()
        );
    });

    let (prefix, config) = test_config(&gateway.store_config(upstream, port), &[]);
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET /stored HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("gateway upstream should receive the request before the client reset")
        .unwrap();

    let mut response = Vec::new();
    read_until(&mut client, &mut response, b"first").await;
    reset_client(client);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after draining the gateway store")
        .unwrap();
    backend.await.unwrap();
    let stored = prefix.join("html/stored");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if stored.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the preserved gateway store should publish its file");
    assert_eq!(std::fs::read(stored).unwrap(), b"firstsecond");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn assert_gateway_cache_survives_client_abort(gateway: RawGateway, port: u16) {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        gateway.read_request(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 11\r\n\r\nfirst",
            )
            .await
            .unwrap();
        stream.flush().await.unwrap();
        release_rx.await.unwrap();
        stream.write_all(b"second").await.unwrap();
        stream.flush().await.unwrap();
    });

    let (prefix, config) = test_config(&gateway.cache_config(upstream, port), &[]);
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET /cached HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("gateway upstream should receive the request before the client reset")
        .unwrap();

    let mut response = Vec::new();
    read_until(&mut client, &mut response, b"first").await;
    reset_client(client);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after draining the gateway cache tee")
        .unwrap();
    backend.await.unwrap();
    let files = wait_for_durable_files(&prefix.join("cache")).await;
    assert_eq!(
        files.len(),
        1,
        "gateway cache tee should publish the complete entry after the client aborts"
    );

    let cached = request_body_with_state(
        &config,
        test_state(&config),
        "GET /cached HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(cached.as_bytes());
    assert!(headers.contains("X-Cache-Status: HIT\r\n"), "{headers}");
    assert_eq!(body, b"firstsecond");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn wait_for_durable_files(root: &Path) -> Vec<PathBuf> {
    for _ in 0..100 {
        let files = durable_files(root);
        if !files.is_empty() {
            return files;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Vec::new()
}

fn durable_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(durable_files(&path));
        } else if path
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().starts_with('.'))
        {
            files.push(path);
        }
    }
    files
}
