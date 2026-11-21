use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[tokio::test]
async fn evaluates_stream_proxy_ssl_name_variables() {
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
    let server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);

    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream proxy TLS name test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind stream proxy TLS name backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (stream, _) = backend.accept().await.unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(server_config));
        let mut stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(
            stream.get_ref().1.server_name(),
            Some("dynamic.example.test")
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
                    listen 127.0.0.1:15470;
                    server_name dynamic.example.test;
                    proxy_pass {backend_address};
                    proxy_ssl on;
                    proxy_ssl_server_name on;
                    proxy_ssl_name $server_name;
                    proxy_half_close on;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45691".parse().unwrap();
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
