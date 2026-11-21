use super::support::*;
use super::*;

#[tokio::test]
async fn applies_http_tcp_nodelay_from_the_matched_location() {
    for (location, expected) in [("/off", false), ("/on", true)] {
        let listener = match TcpListener::bind("127.0.0.1:0").await {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping HTTP tcp_nodelay runtime test: sockets not permitted");
                return;
            }
            Err(error) => panic!("bind HTTP tcp_nodelay listener: {error}"),
        };
        let address = listener.local_addr().unwrap();
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen {address};
                        location /off {{
                            tcp_nodelay off;
                            return 204;
                        }}
                        location /on {{
                            tcp_nodelay on;
                            return 204;
                        }}
                    }}
                }}
            "#
        );
        let (prefix, config) = test_config(&source, &[]);
        let group = build_listener_groups(&config)[0].clone();
        let state = test_state(&config);
        let server = tokio::spawn(async move {
            let (stream, remote) = listener.accept().await.unwrap();
            let (stream, control) =
                ClientSocketControl::attach_tcp(stream).expect("attach socket control");
            let control = control.expect("duplicate TCP socket handle");
            let observed = control.clone();
            handle_http_connection_with_reset(
                stream,
                ConnectionAddresses::direct(
                    ListenAddress::Inet(remote),
                    ListenAddress::Inet(address),
                ),
                group,
                state,
                None,
                Some(control),
            )
            .await;
            observed
        });

        let mut client = TcpStream::connect(address).await.unwrap();
        client
            .write_all(
                format!("GET {location} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                    .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(
            response.starts_with(b"HTTP/1.1 204 No Content\r\n"),
            "{}",
            String::from_utf8_lossy(&response)
        );
        let control = server.await.unwrap();
        assert_eq!(control.enabled().unwrap(), expected, "{location}");
        let _ = std::fs::remove_dir_all(prefix);
    }
}

#[tokio::test]
async fn preserves_tls_nodelay_enabled_by_the_default_server() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping TLS tcp_nodelay runtime test: sockets not permitted");
            return;
        }
        Err(error) => panic!("bind TLS tcp_nodelay listener: {error}"),
    };
    let address = listener.local_addr().unwrap();
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
                    listen {address} ssl;
                    server_name localhost;
                    tcp_nodelay on;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    location /off {{
                        tcp_nodelay off;
                        return 204;
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    *state.tls_configs.write().await =
        build_tls_configs(&config, std::slice::from_ref(&group)).unwrap();
    let server = tokio::spawn(async move {
        let (stream, remote) = listener.accept().await.unwrap();
        let (stream, control) =
            ClientSocketControl::attach_tcp(stream).expect("attach socket control");
        let control = control.expect("duplicate TCP socket handle");
        let observed = control.clone();
        handle_http_connection_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), ListenAddress::Inet(address)),
            group,
            state,
            None,
            Some(control),
        )
        .await;
        observed
    });

    let mut roots = RootCertStore::empty();
    let mut certificate_reader = StdBufReader::new(File::open(ca_certificate).unwrap());
    for certificate in rustls_pemfile::certs(&mut certificate_reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let client = TcpStream::connect(address).await.unwrap();
    let mut client = connector.connect(domain, client).await.unwrap();
    client
        .write_all(b"GET /off HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    let _ = client.read_to_end(&mut response).await;
    assert!(
        response.starts_with(b"HTTP/1.1 204 No Content\r\n"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    let control = server.await.unwrap();
    assert!(control.enabled().unwrap());
    let _ = std::fs::remove_dir_all(prefix);
}
