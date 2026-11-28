use super::support::*;
use super::*;
use bytes::Buf;
use bytes::Bytes;

#[tokio::test]
async fn serves_http3_get_and_streams_request_body() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                http3 on;
                server {{
                    listen 127.0.0.1:0 quic;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    location /get {{
                        add_header X-Http3 $http3 always;
                        add_header X-H3-Alpn $ssl_alpn_protocol always;
                        return 200 "get:$server_protocol:$scheme:$https";
                    }}
                    location /post {{
                        add_header X-Request-Body $request_body always;
                        return 200 "posted";
                    }}
                    location /trailers {{
                        add_trailer X-Response-Trailer ready always;
                        return 200 "trailers";
                    }}
                    location /bad-body {{
                        add_header X-Request-Body $request_body always;
                        return 200 "unexpected";
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let groups = build_listener_groups(&config);
    assert_eq!(groups.len(), 1);
    let group = groups[0].clone();
    assert!(group.quic);
    assert!(!group.tcp);
    let tls_configs = build_tls_configs(&config, &groups)
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let server_config = build_http3_server_config(&config, &group, tls_configs).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let server_address = endpoint.local_addr().unwrap();
    let state = test_state(&config);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(http3_accept_loop(endpoint, group, state, shutdown_rx));

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let mut client_crypto =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
    ));
    let mut client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);
    let connection = client_endpoint
        .connect(server_address, "localhost")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, mut send_request) = h3::client::builder()
        .build::<_, _, Bytes>(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    let driver_task =
        tokio::spawn(
            async move { std::future::poll_fn(|context| driver.poll_close(context)).await },
        );

    let mut request = send_request
        .send_request(HttpRequest::get("https://localhost/get").body(()).unwrap())
        .await
        .unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-http3").unwrap(), "h3");
    assert_eq!(response.headers().get("x-h3-alpn").unwrap(), "h3");
    assert!(response.headers().get("alt-svc").is_none());
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(std::str::from_utf8(&body).unwrap(), "get:HTTP/3.0:https:on");
    drop(request);

    let mut request = send_request
        .send_request(
            HttpRequest::post("https://localhost/post")
                .header(http::header::CONTENT_LENGTH, "5")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    request
        .send_data(Bytes::from_static(b"hello"))
        .await
        .unwrap();
    request.finish().await.unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("x-request-body").unwrap(), "hello");
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, b"posted");
    drop(request);

    let mut request = send_request
        .send_request(
            HttpRequest::get("https://localhost/trailers")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, b"trailers");
    let trailers = request.recv_trailers().await.unwrap().unwrap();
    assert_eq!(trailers.get("x-response-trailer").unwrap(), "ready");
    drop(request);

    let mut request = send_request
        .send_request(
            HttpRequest::post("https://localhost/bad-body")
                .header(http::header::CONTENT_LENGTH, "5")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    request
        .send_data(Bytes::from_static(b"four"))
        .await
        .unwrap();
    request.finish().await.unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    drop(request);

    client_endpoint.close(quinn::VarInt::from_u32(0), b"done");
    let _ = tokio::time::timeout(Duration::from_secs(2), driver_task).await;
    shutdown.send(ShutdownMode::Graceful).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn forwards_http3_request_trailers_and_sends_informational_responses() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");

    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/forward");
        let work = async move {
            let mut body = request.into_body();
            let mut received = Vec::new();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                received.extend_from_slice(&chunk);
                body.flow_control().release_capacity(chunk.len()).unwrap();
            }
            let trailers = body.trailers().await.unwrap().unwrap();
            assert_eq!(trailers.get("x-request-trailer").unwrap(), "done");
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .body(())
                .unwrap();
            let mut send = respond.send_response(response, false).unwrap();
            send.send_data(Bytes::from_static(b"forwarded"), true)
                .unwrap();
            received
        };
        tokio::pin!(work);
        let received = loop {
            tokio::select! {
                received = &mut work => break received,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        break work.await;
                    }
                }
            }
        };
        assert_eq!(received, b"payload");
        connection.graceful_shutdown();
        while connection.accept().await.is_some() {}

        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/hints");
        let work = async move {
            let informational = http::Response::builder()
                .status(http::StatusCode::EARLY_HINTS)
                .header("link", "</style.css>; rel=preload")
                .body(())
                .unwrap();
            respond.send_informational(informational).unwrap();
            let response = http::Response::builder()
                .status(200)
                .header("content-type", "text/plain")
                .body(())
                .unwrap();
            let mut send = respond.send_response(response, false).unwrap();
            send.send_data(Bytes::from_static(b"hinted"), true).unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                _ = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        connection.graceful_shutdown();
        while connection.accept().await.is_some() {}
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                http3 on;
                upstream test_backend {{
                    server {upstream_address};
                }}
                server {{
                    listen 127.0.0.1:0 quic;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    location /forward {{
                        proxy_pass http://test_backend;
                        proxy_http_version 2;
                    }}
                    location /hints {{
                        early_hints $http_x_want_hints;
                        proxy_pass http://test_backend;
                        proxy_http_version 2;
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let groups = build_listener_groups(&config);
    let group = groups[0].clone();
    let tls_configs = build_tls_configs(&config, &groups)
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let server_config = build_http3_server_config(&config, &group, tls_configs).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let server_address = endpoint.local_addr().unwrap();
    let state = test_state(&config);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(http3_accept_loop(endpoint, group, state, shutdown_rx));

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let mut client_crypto =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
    ));
    let mut client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);
    let connection = client_endpoint
        .connect(server_address, "localhost")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, mut send_request) = h3::client::builder()
        .build::<_, _, Bytes>(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    let driver_task =
        tokio::spawn(
            async move { std::future::poll_fn(|context| driver.poll_close(context)).await },
        );

    let mut request = send_request
        .send_request(
            HttpRequest::post("https://localhost/forward")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    request
        .send_data(Bytes::from_static(b"payload"))
        .await
        .unwrap();
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-request-trailer", http::HeaderValue::from_static("done"));
    request.send_trailers(trailers).await.unwrap();
    request.finish().await.unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, b"forwarded");
    drop(request);

    let mut request = send_request
        .send_request(
            HttpRequest::get("https://localhost/hints")
                .header("x-want-hints", "yes")
                .body(())
                .unwrap(),
        )
        .await
        .unwrap();
    let informational = request.recv_response().await.unwrap();
    assert_eq!(informational.status(), StatusCode::EARLY_HINTS);
    assert_eq!(
        informational.headers().get("link").unwrap(),
        "</style.css>; rel=preload"
    );
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(body, b"hinted");
    drop(request);

    backend.await.unwrap();
    client_endpoint.close(quinn::VarInt::from_u32(0), b"done");
    let _ = tokio::time::timeout(Duration::from_secs(2), driver_task).await;
    shutdown.send(ShutdownMode::Graceful).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_http3_client_certificate_and_enforces_online_ocsp() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let server_certificate = fixture.join("test.crt");
    let server_key = fixture.join("test.key");
    let server_ca_certificate = fixture.join("ca.crt");
    let ocsp_fixture = fixture.join("ocsp");
    let client_certificate = ocsp_fixture.join("server.crt");
    let client_key = ocsp_fixture.join("server.key");
    let client_ca_certificate = ocsp_fixture.join("ca.crt");
    let ocsp_response = std::fs::read(ocsp_fixture.join("response.der")).unwrap();

    let responder = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let responder_address = responder.local_addr().unwrap();
    let responder_response = ocsp_response.clone();
    let responder_task = tokio::spawn(async move {
        let (mut stream, _) = responder.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let length = stream.read(&mut buffer).await.unwrap();
            assert_ne!(length, 0);
            request.extend_from_slice(&buffer[..length]);
        }
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
            responder_response.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&responder_response).await.unwrap();
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                http3 on;
                server {{
                    listen 127.0.0.1:0 quic;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.3;
                    ssl_verify_client on;
                    ssl_client_certificate {};
                    ssl_ocsp leaf;
                    ssl_ocsp_responder http://{responder_address}/ocsp;
                    location / {{
                        return 200 "client:$ssl_client_verify:$ssl_client_fingerprint:$ssl_client_s_dn_legacy";
                    }}
                }}
            }}
        "#,
        server_certificate.display(),
        server_key.display(),
        client_ca_certificate.display(),
    );
    let (prefix, config) = test_config(&source, &[]);
    let groups = build_listener_groups(&config);
    let group = groups[0].clone();
    let tls_configs = build_tls_configs(&config, &groups)
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let server_config = build_http3_server_config(&config, &group, tls_configs.clone()).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let server_address = endpoint.local_addr().unwrap();
    let state = test_state(&config);
    state
        .tls_configs
        .write()
        .await
        .insert(group.address.clone(), tls_configs);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(http3_accept_loop(endpoint, group, state, shutdown_rx));

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(server_ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_certificates = crate::server::tls::load_certificates(&client_certificate).unwrap();
    let client_private_key = crate::server::tls::load_private_key(&client_key, &[]).unwrap();
    let mut client_crypto =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_client_auth_cert(client_certificates, client_private_key)
            .unwrap();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
    ));
    let mut client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client_endpoint.set_default_client_config(client_config);
    let connection = client_endpoint
        .connect(server_address, "localhost")
        .unwrap()
        .await
        .unwrap();
    let (mut driver, mut send_request) = h3::client::builder()
        .build::<_, _, Bytes>(h3_quinn::Connection::new(connection))
        .await
        .unwrap();
    let driver_task =
        tokio::spawn(
            async move { std::future::poll_fn(|context| driver.poll_close(context)).await },
        );

    let mut request = send_request
        .send_request(HttpRequest::get("https://localhost/").body(()).unwrap())
        .await
        .unwrap();
    let response = request.recv_response().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = Vec::new();
    while let Some(chunk) = request.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(
        std::str::from_utf8(&body).unwrap(),
        "client:SUCCESS:260db583bbb287669bc40cf3553bd59d3e5d08ec:/CN=localhost"
    );

    drop(request);
    responder_task.await.unwrap();
    client_endpoint.close(quinn::VarInt::from_u32(0), b"done");
    let _ = tokio::time::timeout(Duration::from_secs(2), driver_task).await;
    shutdown.send(ShutdownMode::Graceful).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn explicit_http3_off_suppresses_implicit_quic_listener() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18470 quic;
                    http3 off;
                }
                server {
                    listen 127.0.0.1:18471 quic;
                }
            }
        "#,
        &[],
    );
    let groups = build_listener_groups(&config);
    assert_eq!(groups.len(), 1);
    assert_eq!(
        groups[0].address,
        ListenAddress::Inet("127.0.0.1:18471".parse().unwrap())
    );
    assert!(groups[0].quic);
    assert!(!groups[0].tcp);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn inherited_http3_off_suppresses_quic_listener() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                http3 off;
                server {
                    listen 127.0.0.1:18473 quic;
                }
            }
        "#,
        &[],
    );
    assert!(build_listener_groups(&config).is_empty());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn injects_alt_svc_only_for_non_http3_responses_without_an_existing_header() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                http3 on;
                server {
                    listen 127.0.0.1:18472;
                    listen 127.0.0.1:18472 quic;
                    location /auto {
                        return 200 "ok";
                    }
                    location /custom {
                        add_header Alt-Svc 'h3=":8443"; ma=60' always;
                        return 200 "ok";
                    }
                }
            }
        "#,
        &[],
    );
    let groups = build_listener_groups(&config);
    assert_eq!(groups.len(), 1);
    assert!(groups[0].tcp);
    assert!(groups[0].quic);

    let response = request_body(
        &config,
        "GET /auto HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let lowercase = response.to_ascii_lowercase();
    assert_eq!(
        lowercase.matches("alt-svc:").count(),
        1,
        "unexpected response headers: {response}"
    );
    assert!(lowercase.contains("alt-svc: h3=\":18472\"; ma=86400\r\n"));

    let response = request_body(
        &config,
        "GET /custom HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let lowercase = response.to_ascii_lowercase();
    assert_eq!(
        lowercase.matches("alt-svc:").count(),
        1,
        "unexpected response headers: {response}"
    );
    assert!(lowercase.contains("alt-svc: h3=\":8443\"; ma=60\r\n"));
    assert!(!lowercase.contains("ma=86400"));

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reload_replaces_quic_server_config_and_closes_removed_endpoint() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let old_source = format!(
        r#"
            auto_https off;
            events {{}}
            http {{
                http3 on;
                server {{
                    listen 127.0.0.1:0 quic;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                }}
            }}
        "#,
        fixture.join("test.crt").display(),
        fixture.join("test.key").display()
    );
    let new_source = format!(
        r#"
            auto_https off;
            events {{}}
            http {{
                http3 on;
                server {{
                    listen 127.0.0.1:0 quic;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                }}
            }}
        "#,
        fixture.join("crl-server.crt").display(),
        fixture.join("crl-server.key").display()
    );
    let off_source = r#"
        auto_https off;
        events {}
        http {
            server {
                listen 127.0.0.1:0 quic;
                http3 off;
            }
        }
    "#;
    let (prefix, old_config) = test_config(&old_source, &[]);
    let old_groups = build_listener_groups(&old_config);
    let old_group = old_groups[0].clone();
    let old_tls = build_tls_configs(&old_config, &old_groups)
        .unwrap()
        .remove(&old_group.address)
        .unwrap();
    let server_config = build_http3_server_config(&old_config, &old_group, old_tls).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(server_config),
        socket,
        Arc::new(quinn::TokioRuntime),
    )
    .unwrap();
    let server_address = endpoint.local_addr().unwrap();
    let state = test_state(&old_config);
    state
        .http3_endpoints
        .lock()
        .unwrap()
        .insert(old_group.address.clone(), endpoint.clone());
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(http3_accept_loop(
        endpoint.clone(),
        old_group.clone(),
        state.clone(),
        shutdown_rx,
    ));

    let config_path = prefix.join("conf/nginx.conf");
    std::fs::write(&config_path, new_source).unwrap();
    crate::server::run::reload(&state, &config_path, &prefix).await;
    let new_result = http3_handshake(server_address, &fixture.join("crl-ca.crt")).await;
    assert!(new_result.is_ok(), "{new_result:?}");
    assert!(
        http3_handshake(server_address, &fixture.join("ca.crt"))
            .await
            .is_err()
    );

    std::fs::write(&config_path, off_source).unwrap();
    crate::server::run::reload(&state, &config_path, &prefix).await;
    assert!(state.http3_endpoints.lock().unwrap().is_empty());

    std::fs::write(&config_path, old_source).unwrap();
    crate::server::run::reload(&state, &config_path, &prefix).await;
    assert!(state.http3_endpoints.lock().unwrap().is_empty());
    assert!(build_listener_groups(&state.current_config()).is_empty());

    let _ = shutdown.send(ShutdownMode::Graceful);
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;

    let _ = std::fs::remove_dir_all(prefix);
}

async fn http3_handshake(server_address: SocketAddr, ca_certificate: &Path) -> Result<(), String> {
    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let mut client_crypto =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto).unwrap(),
    ));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(client_config);
    let result = endpoint
        .connect(server_address, "localhost")
        .map_err(|error| error.to_string())?
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    endpoint.close(quinn::VarInt::from_u32(0), b"done");
    result
}
