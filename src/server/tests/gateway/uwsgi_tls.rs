use super::*;

#[test]
fn resolves_dynamic_uwsgi_schemes_before_target_selection() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18510;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18510".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18510".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    for (raw, expected_tls) in [
        ("suwsgi://127.0.0.1:9001", true),
        ("sUwSgI://127.0.0.1:9001", true),
        ("uwsgi://127.0.0.1:9001", false),
    ] {
        let pass = UwsgiPassConfig {
            raw: raw.into(),
            tls: false,
        };
        let (targets, tls) =
            resolve_uwsgi_targets(&pass, config.http.as_ref().unwrap(), &variables, None);
        assert_eq!(tls, expected_tls, "{raw}");
        assert_eq!(targets.len(), 1, "{raw}");
        assert_eq!(targets[0].host, "127.0.0.1", "{raw}");
        assert_eq!(targets[0].port, 9001, "{raw}");
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn negotiates_suwsgi_tls_and_exchanges_a_packet() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certified = load_certificate_paths(
        Some(fixture.join("test.crt").as_path()),
        Some(fixture.join("test.key").as_path()),
        &[],
    )
    .unwrap();
    let resolver = Arc::new(CertificateResolver {
        exact: HashMap::new(),
        wildcard: Vec::new(),
        default: certified,
        stapling: None,
    });
    let versions = rustls_protocol_versions(TlsProtocols::default());
    let server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18511;
                    location / {
                        uwsgi_pass suwsgi://127.0.0.1:9001;
                        uwsgi_param REQUEST_METHOD $request_method;
                        uwsgi_ssl_server_name on;
                        uwsgi_ssl_name localhost;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "GET".into(),
        target: "/secure".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18511".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18511".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);

    let (client, server_io) = tokio::io::duplex(64 * 1024);
    let backend = tokio::spawn(async move {
        let mut stream = acceptor.accept(server_io).await.unwrap();
        assert_eq!(stream.get_ref().1.server_name(), Some("localhost"));
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 0);
        let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut parameters = vec![0_u8; parameter_length];
        stream.read_exact(&mut parameters).await.unwrap();
        let values = parse_null_separated_params(&parameters);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("GET")
        );
        stream
            .write_all(b"Status: 202 Accepted\r\nContent-Type: text/plain\r\n\r\nsuwsgi-ok")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });

    let tls_stream = upgrade_uwsgi_tls(client, "127.0.0.1", context, &variables)
        .await
        .unwrap();
    let response = uwsgi_exchange(tls_stream, &request, context, &variables)
        .await
        .unwrap();
    assert_eq!(response.status, 202);
    assert_eq!(response.into_bytes(), b"suwsgi-ok");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
