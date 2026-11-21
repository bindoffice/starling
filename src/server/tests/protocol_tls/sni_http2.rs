use super::*;

#[tokio::test]
async fn selects_http_tls_configuration_by_sni() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18451 ssl default_server;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.2;
                        location / {{ return 200 "default\n"; }}
                    }}
                    server {{
                        listen 127.0.0.1:18451 ssl;
                        server_name localhost;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        ssl_protocols TLSv1.3;
                        location / {{ return 200 "named\n"; }}
                    }}
                }}
            "#,
        certificate.display(),
        key.display(),
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let groups = build_listener_groups(&config);
    assert_eq!(groups.len(), 1);
    let group = groups[0].clone();
    let state = test_state(&config);
    *state.tls_configs.write().await =
        build_tls_configs(&config, std::slice::from_ref(&group)).unwrap();

    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45680".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(
        client.get_ref().1.protocol_version(),
        Some(tokio_rustls::rustls::ProtocolVersion::TLSv1_3)
    );
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    if let Err(err) = client.read_to_end(&mut response).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("named\n"), "{response}");
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn negotiates_http2_with_alpn_and_serves_request() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let ca_certificate = fixture.join("ca.crt");
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18444 ssl http2;
                        server_name localhost;
                        ssl_certificate {};
                        ssl_certificate_key {};
                        root html;
                        max_headers 5;
                        server_tokens build;
                        add_header Connection keep-alive always;
                        add_header X-Protocol h2 always;
                        add_trailer X-Status $status always;
                        location = /h2 {{ return 201 "$request_method:$scheme:$https\n"; }}
                    }}
                }}
            "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[("html/h2.txt", "static-h2\n")]);
    let group = build_listener_groups(&config)[0].clone();
    assert!(group.ssl);
    assert!(group.http2);
    let server_config = build_tls_configs(&config, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();
    assert_eq!(
        server_config.default.alpn_protocols,
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    );
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45681".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let stream = accept_http_tls(server_io, server_config).await.unwrap();
        assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        handle_tls_stream(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
        )
        .await;
    });

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let mut client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    client_config.alpn_protocols = vec![b"h2".to_vec()];
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let tls = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));

    let (mut sender, connection) = h2::client::handshake(tls).await.unwrap();
    let driver = tokio::spawn(async move {
        connection.await.unwrap();
    });
    let request = http::Request::builder()
        .method("POST")
        .uri("https://localhost/h2")
        .header("content-length", "5")
        .body(())
        .unwrap();
    let (response, mut request_body) = sender.send_request(request, false).unwrap();
    request_body
        .send_data(Bytes::from_static(b"hello"), true)
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.version(), http::Version::HTTP_2);
    assert_eq!(
        response
            .headers()
            .get("server")
            .and_then(|value| value.to_str().ok()),
        Some(crate::SERVER_SOFTWARE_BUILD)
    );
    assert!(response.headers().get("connection").is_none());
    assert_eq!(
        response
            .headers()
            .get("x-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("h2")
    );
    let mut body = Vec::new();
    let mut response_body = response.into_body();
    while let Some(data) = response_body.data().await {
        let data = data.unwrap();
        response_body
            .flow_control()
            .release_capacity(data.len())
            .unwrap();
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"POST:https:on\n");
    let trailers = response_body.trailers().await.unwrap().unwrap();
    assert_eq!(
        trailers
            .get("x-status")
            .and_then(|value| value.to_str().ok()),
        Some("201")
    );

    let request = http::Request::builder()
        .uri("https://localhost/h2.txt")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = Vec::new();
    let mut response_body = response.into_body();
    while let Some(data) = response_body.data().await {
        let data = data.unwrap();
        response_body
            .flow_control()
            .release_capacity(data.len())
            .unwrap();
        body.extend_from_slice(&data);
    }
    assert_eq!(body, b"static-h2\n");
    let trailers = response_body.trailers().await.unwrap().unwrap();
    assert_eq!(
        trailers
            .get("x-status")
            .and_then(|value| value.to_str().ok()),
        Some("200")
    );

    let request = http::Request::builder()
        .uri("https://localhost/h2.txt")
        .header("range", "bytes=0-1,3-4")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap()
        .to_string();
    let boundary = content_type
        .strip_prefix("multipart/byteranges; boundary=")
        .expect("multipart content type");
    let mut body = Vec::new();
    let mut response_body = response.into_body();
    while let Some(data) = response_body.data().await {
        let data = data.unwrap();
        response_body
            .flow_control()
            .release_capacity(data.len())
            .unwrap();
        body.extend_from_slice(&data);
    }
    let expected = format!(
        "\r\n--{boundary}\r\nContent-Type: text/plain\r\n\
         Content-Range: bytes 0-1/10\r\n\r\nst\
         \r\n--{boundary}\r\nContent-Type: text/plain\r\n\
         Content-Range: bytes 3-4/10\r\n\r\nti\
         \r\n--{boundary}--\r\n"
    );
    assert_eq!(body, expected.as_bytes());
    let trailers = response_body.trailers().await.unwrap().unwrap();
    assert_eq!(
        trailers
            .get("x-status")
            .and_then(|value| value.to_str().ok()),
        Some("206")
    );

    let request = http::Request::builder()
        .uri("https://localhost/h2.txt")
        .header("x-one", "1")
        .header("x-two", "2")
        .header("x-three", "3")
        .header("x-four", "4")
        .header("x-five", "5")
        .header("x-six", "6")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
