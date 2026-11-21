use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_proxy_ssl_alpn() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_ssl_alpn h2 http/1.1;
                server {
                    listen 127.0.0.1:15455;
                    proxy_pass 127.0.0.1:15432;
                }
                server {
                    listen 127.0.0.1:15456;
                    proxy_ssl_alpn postgres;
                    proxy_pass 127.0.0.1:15432;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_ssl_alpn, ["h2", "http/1.1"]);
    assert_eq!(servers[1].proxy_ssl_alpn, ["postgres"]);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_proxy_ssl_alpn_directives() {
    let main_duplicate = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_ssl_alpn h2;
                proxy_ssl_alpn http/1.1;
                server {
                    listen 127.0.0.1:15457;
                    proxy_pass 127.0.0.1:15432;
                }
            }
        "#,
    );
    assert!(main_duplicate.contains("\"proxy_ssl_alpn\" directive is duplicate"));

    let server_duplicate = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:15458;
                    proxy_ssl_alpn h2;
                    proxy_ssl_alpn http/1.1;
                    proxy_pass 127.0.0.1:15432;
                }
            }
        "#,
    );
    assert!(server_duplicate.contains("\"proxy_ssl_alpn\" directive is duplicate"));

    let too_long = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15459;
                    proxy_ssl_alpn {};
                    proxy_pass 127.0.0.1:15432;
                }}
            }}
        "#,
        "x".repeat(256)
    );
    let error = test_config_error(&too_long);
    assert!(error.contains("protocol too long in \"proxy_ssl_alpn\" directive"));
}

#[tokio::test]
async fn negotiates_stream_proxy_ssl_alpn_with_the_upstream() {
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
    let versions = rustls_protocol_versions(TlsProtocols::default());
    let mut server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    server_config.alpn_protocols = vec![b"database".to_vec()];

    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream proxy ALPN test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind stream upstream TLS backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (stream, _) = backend.accept().await.unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let mut stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(
            stream.get_ref().1.alpn_protocol(),
            Some(b"database".as_slice())
        );
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15460;
                    set $requested database;
                    proxy_pass {backend_address};
                    proxy_ssl on;
                    proxy_ssl_alpn $requested;
                    proxy_half_close on;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45687".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    client.write_all(b"ping").await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"pong");
    task.await.unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
