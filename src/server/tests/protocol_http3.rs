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
