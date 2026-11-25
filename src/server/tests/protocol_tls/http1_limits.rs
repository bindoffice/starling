use super::*;

#[tokio::test]
async fn handles_expect_continue_and_pipelined_keepalive_requests() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18085;
                        location / { return 200 "ok\n"; }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let logger = Arc::new(Logger::new(&config.core.error_logs, &config.prefix).unwrap());
    let http_error_loggers = build_http_error_loggers(&config).unwrap();
    let (reload, _) = watch::channel(0_u64);
    let state = SharedState {
        config: Arc::new(ArcSwap::from_pointee(RuntimeSnapshot {
            config: config.clone(),
            listener_groups: build_listener_groups(&config).into(),
            http_error_loggers,
            upstream_keepalive_generation: 0,
        })),
        reload: Arc::new(reload),
        tls_configs: Arc::new(RwLock::new(HashMap::new())),
        stream_tls_configs: Arc::new(RwLock::new(HashMap::new())),
        logger,
        stream_loggers: Arc::new(RwLock::new(
            build_stream_loggers(&config).expect("stream error loggers"),
        )),
        limits: Arc::new(Mutex::new(LimitState::default())),
        stats: Arc::new(ConnectionStats::default()),
        upstream_keepalive: Arc::new(UpstreamKeepalivePool::default()),
        proxy_cache: Arc::new(ProxyCacheRegistry::new(config.http.as_ref())),
        client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
        http2_body_budget: Arc::new(Http2BodyBudget::new()),
    };
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(
            ListenAddress::Inet("127.0.0.1:45680".parse().unwrap()),
            group.address.clone(),
        ),
        group,
        state,
    ));
    client
            .write_all(
                b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nExpect: 100-continue\r\n\r\n",
            )
            .await
            .unwrap();
    let mut interim = [0_u8; 25];
    client.read_exact(&mut interim).await.unwrap();
    assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"test").await.unwrap();
    let mut first_response = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let size = client.read(&mut buffer).await.unwrap();
        assert!(size > 0);
        first_response.extend_from_slice(&buffer[..size]);
        if first_response
            .windows(7)
            .any(|window| window == b"\r\n\r\nok\n")
        {
            break;
        }
    }
    assert!(first_response.starts_with(b"HTTP/1.1 200 OK\r\n"));

    client
            .write_all(
                b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nGET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    let mut responses = Vec::new();
    client.read_to_end(&mut responses).await.unwrap();
    let responses = String::from_utf8(responses).unwrap();
    assert_eq!(responses.matches("HTTP/1.1 200 OK\r\n").count(), 2);
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_http1_pipeline_state_like_nginx() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18135;
                        location /pipe { return 200 "$pipe\n"; }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(
            ListenAddress::Inet("127.0.0.1:45684".parse().unwrap()),
            group.address.clone(),
        ),
        group,
        state,
    ));

    client
        .write_all(b"GET /pipe HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut first = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !first.windows(2).any(|window| window == b".\n") {
        let size = client.read(&mut buffer).await.unwrap();
        assert!(size > 0);
        first.extend_from_slice(&buffer[..size]);
    }

    client
        .write_all(
            b"GET /pipe HTTP/1.1\r\nHost: localhost\r\n\r\nGET /pipe HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut responses = Vec::new();
    client.read_to_end(&mut responses).await.unwrap();
    let responses = String::from_utf8(responses).unwrap();
    assert_eq!(responses.matches(".\n").count(), 1, "{responses}");
    assert_eq!(responses.matches("p\n").count(), 1, "{responses}");
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn closes_keepalive_connection_at_request_limit() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18086;
                        keepalive_requests 1;
                        location / { return 200 "ok\n"; }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(
            ListenAddress::Inet("127.0.0.1:45682".parse().unwrap()),
            group.address.clone(),
        ),
        group,
        state,
    ));
    client
        .write_all(
            b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\nGET / HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await
        .unwrap();
    let mut responses = Vec::new();
    client.read_to_end(&mut responses).await.unwrap();
    let responses = String::from_utf8(responses).unwrap();
    assert_eq!(responses.matches("HTTP/1.1 200 OK\r\n").count(), 1);
    assert!(responses.contains("Connection: close\r\n"));
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn times_out_incomplete_request_body() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18087;
                        client_body_timeout 20ms;
                        location / { return 204; }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(
            ListenAddress::Inet("127.0.0.1:45683".parse().unwrap()),
            group.address.clone(),
        ),
        group,
        state,
    ));
    client
        .write_all(b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 408 "));
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn returns_bad_request_for_malformed_request_headers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18134;
                        location / { return 204; }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: bad host\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn times_out_stalled_response_writes() {
    struct StalledWriter;

    impl AsyncWrite for StalledWriter {
        fn poll_write(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Pending
        }

        fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Pending
        }

        fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    let mut stalled = StalledWriter;
    let error = TimedWriter::new(&mut stalled, Duration::from_millis(10))
        .write_all(b"blocked")
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}
