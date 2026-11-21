use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_ssl_reject_handshake_without_certificate() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:15448 ssl default_server;
                    ssl_reject_handshake on;
                    return rejected;
                }
            }
        "#,
        &[],
    );
    assert!(config.stream.as_ref().unwrap().servers[0].ssl_reject_handshake);
    build_stream_tls_configs(&config).expect("reject-only TLS listener");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_unknown_sni_and_allows_named_stream_servers() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15449 ssl default_server;
                    ssl_reject_handshake on;
                    return rejected;
                }}
                server {{
                    listen 127.0.0.1:15449 ssl;
                    server_name allowed.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    return accepted;
                }}
            }}
        "#,
        fixture.join("test.crt").display(),
        fixture.join("test.key").display()
    );
    let (prefix, config) = test_config(&source, &[]);

    let response = stream_tls_result(&config, "allowed.test").await;
    let mut response = response.expect("allowed SNI should complete TLS handshake");
    let mut body = Vec::new();
    if let Err(err) = response.read_to_end(&mut body).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    assert_eq!(body, b"accepted");

    let rejected = stream_tls_result(&config, "unknown.test").await;
    assert!(rejected.is_err(), "unknown SNI unexpectedly completed TLS");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn stream_tls_result(
    config: &Arc<RuntimeConfig>,
    server_name: &str,
) -> io::Result<tokio_rustls::client::TlsStream<tokio::io::DuplexStream>> {
    let route = build_stream_routes(config)[0].clone();
    let local = route.address.clone();
    let state = test_state(config);
    *state.stream_tls_configs.write().await = build_stream_tls_configs(config).unwrap();
    let transport_remote: SocketAddr = "127.0.0.1:45685".parse().unwrap();
    let (client_io, inbound) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    let client_config = build_client_tls_config(ClientTlsOptions {
        verify: false,
        server_name: true,
        session_reuse: true,
        trusted_certificate: None,
        verify_depth: 1,
        crl: None,
        client_certificate: None,
        client_certificate_key: None,
        client_certificate_passwords: &[],
        protocols: TlsProtocols::default(),
        ciphers: &TlsCipherConfig::default(),
        alpn_protocols: &[],
    })
    .unwrap();
    let connector = TlsConnector::from(client_config);
    let domain = TlsServerName::try_from(server_name.to_string()).unwrap();
    let result = connector.connect(domain, client_io).await;
    task.await.unwrap();
    result
}
