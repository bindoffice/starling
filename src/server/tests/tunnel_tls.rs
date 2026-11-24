use super::support::*;
use super::*;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

fn tls_acceptor() -> TlsAcceptor {
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
    TlsAcceptor::from(Arc::new(server_config))
}

#[tokio::test]
async fn tunnel_pass_https_uses_tls_and_preserves_sni_and_verification() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping TLS tunnel test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind TLS tunnel backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let acceptor = tls_acceptor();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(stream.get_ref().1.server_name(), Some("localhost"));
        let mut request = [0_u8; 5];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"hello");
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream tunnel_tls_backend {{
                    server {backend_address};
                }}
                server {{
                    listen 127.0.0.1:18888;
                    location / {{
                        tunnel_pass https://tunnel_tls_backend;
                        tunnel_connect_timeout 1s;
                        tunnel_next_upstream off;
                        proxy_ssl_server_name on;
                        proxy_ssl_name localhost;
                        proxy_ssl_verify on;
                        proxy_ssl_trusted_certificate {};
                    }}
                }}
            }}
        "#,
        fixture.join("ca.crt").display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_bytes(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\nhello",
    )
    .await;
    let (head, body) = split_http_response(&response);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, b"world");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn tunnel_tls_handshake_timeout_returns_504_before_200() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping TLS tunnel timeout test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind TLS timeout backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18889;
                    location / {{
                        tunnel_pass https://{backend_address};
                        tunnel_connect_timeout 30ms;
                        tunnel_next_upstream off;
                        proxy_ssl_verify off;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let started = std::time::Instant::now();
    let response = request_body(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n",
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "TLS handshake timeout did not win over the default timeout"
    );
    assert!(
        response.starts_with("HTTP/1.1 504 "),
        "unexpected TLS handshake timeout response:\n{response}"
    );
    backend.abort();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn tunnel_tls_verification_failure_returns_502_before_200() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping TLS tunnel verify test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind TLS verify backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let acceptor = tls_acceptor();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let _ = acceptor.accept(stream).await;
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18890;
                    location / {{
                        tunnel_pass https://{backend_address};
                        tunnel_connect_timeout 1s;
                        tunnel_next_upstream off;
                        proxy_ssl_server_name on;
                        proxy_ssl_name localhost;
                        proxy_ssl_verify on;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 502 "),
        "unexpected TLS verification response:\n{response}"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn tunnel_tls_failure_retries_to_the_next_upstream() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let first_backend = tokio::spawn(async move {
        let (stream, _) = first.accept().await.unwrap();
        drop(stream);
    });
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_address = second.local_addr().unwrap();
    let acceptor = tls_acceptor();
    let second_backend = tokio::spawn(async move {
        let (stream, _) = second.accept().await.unwrap();
        let mut stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(stream.get_ref().1.server_name(), Some("localhost"));
        let mut request = [0_u8; 5];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"hello");
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream tunnel_tls_backend {{
                    server {first_address};
                    server {second_address};
                }}
                server {{
                    listen 127.0.0.1:18891;
                    location / {{
                        tunnel_pass https://tunnel_tls_backend;
                        tunnel_connect_timeout 500ms;
                        tunnel_next_upstream error timeout;
                        tunnel_next_upstream_tries 2;
                        proxy_ssl_server_name on;
                        proxy_ssl_name localhost;
                        proxy_ssl_verify on;
                        proxy_ssl_trusted_certificate {};
                    }}
                }}
            }}
        "#,
        fixture.join("ca.crt").display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_bytes(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\nhello",
    )
    .await;
    let (head, body) = split_http_response(&response);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, b"world");
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
