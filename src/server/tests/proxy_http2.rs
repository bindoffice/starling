use super::support::*;
use super::*;

mod prefetch;
mod request_errors;
mod request_retry;
mod request_retry_cancel;
mod request_streaming;
mod streaming;
mod streaming_errors;

#[tokio::test]
async fn proxies_http2_early_hints_over_duplex() {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/early");

        let informational = http::Response::builder()
            .status(http::StatusCode::EARLY_HINTS)
            .header("Link", "</style.css>; rel=preload")
            .body(())
            .unwrap();
        respond.send_informational(informational).unwrap();

        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream.send_data(Bytes::from_static(b"ok"), true).unwrap();
        shutdown_h2(connection).await;
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18529;
                    location / {
                        proxy_pass http://backend.test/;
                        proxy_http_version 2;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/early".into(),
        headers: vec![("Host".into(), "backend.test".into())],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let mut connection = ProxyV2Connection::connect(
        Box::new(client_io),
        http2_test_key(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let (response, _) =
        timed_proxy_v2_exchange(&mut connection, &outgoing, proxy, context, 0, false)
            .await
            .unwrap();

    backend.await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.early_hints.len(), 1);
    assert!(response.early_hints[0][0].name.eq_ignore_ascii_case("link"));
    assert_eq!(
        response.early_hints[0][0].value,
        "</style.css>; rel=preload"
    );
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"ok"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_http2_upstream_connections_from_the_keepalive_pool() {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let (close_tx, close_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        for (path, body) in [("/one", "first"), ("/two", "second")] {
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.uri().path(), path);
            let response = http::Response::builder()
                .status(200)
                .header("Content-Type", "text/plain")
                .body(())
                .unwrap();
            let mut stream = respond.send_response(response, false).unwrap();
            stream
                .send_data(Bytes::copy_from_slice(body.as_bytes()), true)
                .unwrap();
        }
        tokio::select! {
            _ = close_rx => {}
            _ = connection.accept() => {}
        }
        shutdown_h2(connection).await;
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                upstream backend {
                    server 127.0.0.1:1;
                    keepalive 2;
                }
                server {
                    listen 127.0.0.1:18528;
                    location / {
                        proxy_pass http://backend/;
                        proxy_http_version 2;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let keepalive = UpstreamKeepaliveConfig::default();
    let key = http2_test_key();
    let pool = UpstreamKeepalivePool::default();
    let connection =
        ProxyV2Connection::connect(Box::new(client_io), key.clone(), Duration::from_secs(1))
            .await
            .unwrap();
    pool.put_http2("backend", &keepalive, connection);
    assert_eq!(pool.len("backend"), 1);

    for (path, expected) in [("/one", "first"), ("/two", "second")] {
        let mut connection = pool
            .take_http2("backend", &key, &keepalive)
            .expect("reused HTTP/2 connection");
        let outgoing = ProxyRequestMessage {
            method: "GET".into(),
            path: path.into(),
            headers: vec![("Host".into(), "backend.test".into())],
            body: Vec::new(),
            body_stream: None,
            body_chunked: false,
            body_limit: 0,
        };
        let (response, _) =
            timed_proxy_v2_exchange(&mut connection, &outgoing, proxy, context, 0, false)
                .await
                .unwrap();
        assert_eq!(
            response
                .body
                .into_memory(MAX_BODY_BYTES as u64)
                .await
                .unwrap(),
            expected.as_bytes()
        );
        pool.put_http2("backend", &keepalive, connection);
    }

    assert_eq!(pool.len("backend"), 1);
    drop(pool);
    let _ = close_tx.send(());
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxies_http2_requests_with_bodies_and_trailers() {
    let Some(upstream) = bind_test_listener("HTTP/2 proxy request body test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            assert_eq!(request.method(), http::Method::POST);
            assert_eq!(request.uri().path(), "/echo");
            assert_eq!(
                request
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok()),
                Some("text/plain")
            );
            let mut body = Vec::new();
            let mut request_body = request.into_body();
            while let Some(chunk) = request_body.data().await {
                body.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(body, b"request");

            let response = http::Response::builder()
                .status(200)
                .header("Content-Type", "text/plain")
                .header("X-Upstream", "h2")
                .body(())
                .unwrap();
            let mut stream = respond.send_response(response, false).unwrap();
            stream
                .send_data(Bytes::from_static(b"http2-response"), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("x-trailer", http::HeaderValue::from_static("yes"));
            stream.send_trailers(trailers).unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_h2(connection).await;
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18530;
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_http_version 2;
                            proxy_pass_trailers on;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST /echo HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: 7\r\n\
         Connection: close\r\n\r\n\
         request",
    )
    .await;
    backend.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("x-upstream: h2\r\n"), "{response}");
    assert!(response.contains("http2-response"), "{response}");
    assert!(response.contains("x-trailer: yes\r\n"), "{response}");

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn negotiates_http2_with_tls_upstreams() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let certified =
        load_certificate_paths(Some(certificate.as_path()), Some(key.as_path()), &[]).unwrap();
    let resolver = Arc::new(CertificateResolver {
        exact: HashMap::new(),
        wildcard: Vec::new(),
        default: certified,
        stapling: None,
    });
    let versions = rustls_protocol_versions(TlsProtocols {
        tls12: true,
        tls13: true,
    });
    let mut server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    server_config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let Some(upstream) = bind_test_listener("HTTP/2 proxy TLS test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.method(), http::Method::GET);
        let work = async move {
            let response = http::Response::builder()
                .status(200)
                .header("Content-Type", "text/plain")
                .body(())
                .unwrap();
            let mut stream = respond.send_response(response, false).unwrap();
            stream
                .send_data(Bytes::from_static(b"secure-h2"), true)
                .unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_h2(connection).await;
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18532;
                        location / {{
                            proxy_pass https://{upstream_address}/;
                            proxy_http_version 2;
                            proxy_ssl_name localhost;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "GET /secure HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    backend.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("secure-h2"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn accepts_proxy_http_version_two() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18531;
                location / {
                    proxy_pass http://127.0.0.1:9000;
                    proxy_http_version 2;
                }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    assert_eq!(context.proxy_http_version, "2");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn shutdown_h2(
    mut connection: h2::server::Connection<impl AsyncRead + AsyncWrite + Unpin, Bytes>,
) {
    connection.graceful_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while connection.accept().await.is_some() {}
    })
    .await;
}

async fn bind_test_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: binding TCP sockets is not permitted");
            None
        }
        Err(error) => panic!("bind TCP listener: {error}"),
    }
}

fn http2_test_key() -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: 0,
        scheme: "http".into(),
        host: "backend.test".into(),
        port: 80,
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
        http_version: "2".into(),
    }
}
