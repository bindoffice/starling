use super::*;

fn keepalive_test_key(port: u16) -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: 0,
        scheme: "http".into(),
        host: "127.0.0.1".into(),
        port,
        tls_name: None,
        tls_verify: false,
        tls_trusted_certificate: None,
        tls_verify_depth: None,
        tls_crl: None,
        tls_client_certificate: None,
        tls_client_certificate_key: None,
        tls_protocols: None,
        tls_ciphers: None,
        proxy_bind: None,
        socket_keepalive: false,
        socket_rcvbuf: 0,
        socket_sndbuf: 0,
        http_version: "1.1".into(),
    }
}

#[tokio::test]
async fn upstream_keepalive_pool_enforces_capacity_and_request_limit() {
    let pool = UpstreamKeepalivePool::default();
    let key = keepalive_test_key(8080);
    let mut config = UpstreamKeepaliveConfig {
        max_cached: 2,
        requests: 2,
        time: Duration::from_secs(60),
        timeout: Duration::from_secs(30),
        local: false,
    };
    for _ in 0..3 {
        let (client, _server) = tokio::io::duplex(64);
        pool.put(
            "backend",
            &config,
            UpstreamConnection::new(Box::new(client), key.clone()),
        )
        .await;
    }
    assert_eq!(pool.len("backend").await, 2);

    let (client, _server) = tokio::io::duplex(64);
    let mut exhausted = UpstreamConnection::new(Box::new(client), key.clone());
    exhausted.requests = config.requests;
    pool.put("backend", &config, exhausted).await;
    assert_eq!(pool.len("backend").await, 2);
    assert!(
        pool.take("backend", &key, &config)
            .await
            .is_some_and(|connection| connection.requests < config.requests)
    );

    let (client, _server) = tokio::io::duplex(64);
    let mut expired = UpstreamConnection::new(Box::new(client), key.clone());
    expired.idle_since = Instant::now().checked_sub(Duration::from_secs(31)).unwrap();
    pool.entries
        .lock()
        .await
        .entry("backend".into())
        .or_default()
        .push_front(expired.into());
    while let Some(connection) = pool.take("backend", &key, &config).await {
        assert!(connection.idle_since.elapsed() < config.timeout);
    }
    assert_eq!(pool.len("backend").await, 0);

    config.requests = 0;
    let (client, _server) = tokio::io::duplex(64);
    pool.put(
        "backend",
        &config,
        UpstreamConnection::new(Box::new(client), key),
    )
    .await;
    assert_eq!(pool.len("backend").await, 0);

    pool.clear().await;
    config.requests = 1;
    let stale_key = keepalive_test_key(8080);
    let (client, _server) = tokio::io::duplex(64);
    pool.put(
        "backend",
        &config,
        UpstreamConnection::new(Box::new(client), stale_key),
    )
    .await;
    assert_eq!(pool.len("backend").await, 0);
}

#[tokio::test]
async fn upstream_keepalive_time_applies_when_returning_a_connection() {
    let pool = UpstreamKeepalivePool::default();
    let key = keepalive_test_key(8080);
    let config = UpstreamKeepaliveConfig {
        max_cached: 1,
        requests: 1000,
        time: Duration::from_secs(2),
        timeout: Duration::from_secs(30),
        local: false,
    };
    let (client, _server) = tokio::io::duplex(64);
    let mut connection = UpstreamConnection::new(Box::new(client), key.clone());
    connection.created = Instant::now().checked_sub(Duration::from_secs(3)).unwrap();
    pool.entries
        .lock()
        .await
        .entry("backend".into())
        .or_default()
        .push_front(connection.into());

    let connection = pool
        .take("backend", &key, &config)
        .await
        .expect("a previously pooled connection should be reusable once");
    pool.put("backend", &config, connection).await;
    assert_eq!(pool.len("backend").await, 0);
}

#[tokio::test]
async fn reuses_an_upstream_connection_for_multiple_http_requests() {
    let key = keepalive_test_key(8080);
    let outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/health".into(),
        headers: vec![
            ("Host".into(), "backend".into()),
            ("Connection".into(), "keep-alive".into()),
        ],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let mut reader = BufReader::new(server);
        for body in ["one", "two"] {
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert_eq!(request.matches("GET /health HTTP/1.1").count(), 1);
            reader
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });

    let mut connection = UpstreamConnection::new(Box::new(client), key);
    let (first, _) = exchange_upstream_connection(&mut connection, &outgoing, "1.1")
        .await
        .unwrap();
    assert_eq!(first.body.len(), 3);
    assert_eq!(first.body.into_memory(16).await.unwrap(), b"one");
    assert!(connection.reader.buffer().is_empty());
    let (second, _) = exchange_upstream_connection(&mut connection, &outgoing, "1.1")
        .await
        .unwrap();
    assert_eq!(second.body.len(), 3);
    assert!(can_reuse_proxy_connection(&second, &outgoing, "1.1"));
    server_task.await.unwrap();
}

#[test]
fn does_not_reuse_unframed_or_closed_upstream_responses() {
    let outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/".into(),
        headers: vec![("Connection".into(), "keep-alive".into())],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let reusable = UpstreamResponse {
        status: 200,
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Content-Length".into(),
            value: "2".into(),
        }],
        trailers: Vec::new(),
        early_hints: Vec::new(),
        body: ResponseBody::Bytes(b"ok".to_vec()),
        body_temp_file: None,
    };
    assert!(can_reuse_proxy_connection(&reusable, &outgoing, "1.1"));

    let closed = UpstreamResponse {
        status: 200,
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Content-Length".into(),
                value: "2".into(),
            },
            Header {
                name: "Connection".into(),
                value: "keep-alive, close".into(),
            },
        ],
        trailers: Vec::new(),
        early_hints: Vec::new(),
        body: ResponseBody::Bytes(b"ok".to_vec()),
        body_temp_file: None,
    };
    assert!(!can_reuse_proxy_connection(&closed, &outgoing, "1.1"));

    let unframed = UpstreamResponse {
        status: 200,
        version: "HTTP/1.1".into(),
        headers: Vec::new(),
        trailers: Vec::new(),
        early_hints: Vec::new(),
        body: ResponseBody::Bytes(b"body".to_vec()),
        body_temp_file: None,
    };
    assert!(!can_reuse_proxy_connection(&unframed, &outgoing, "1.1"));
    assert!(!can_reuse_proxy_connection(&reusable, &outgoing, "1.0"));
}
