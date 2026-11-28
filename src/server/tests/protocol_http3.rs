use super::support::*;
use super::*;
use bytes::Buf;

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
        .build(h3_quinn::Connection::new(connection))
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
