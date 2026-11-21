use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[tokio::test]
async fn selects_stream_tls_config_and_virtual_server_by_sni() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let default_certificate = fixture.join("test.crt");
    let default_key = fixture.join("test.key");
    let alternate_certificate = fixture.join("crl-server.crt");
    let alternate_key = fixture.join("crl-server.key");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15444 ssl;
                    server_name default.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    return "default|$ssl_server_name|$server_name";
                }}
                server {{
                    listen 127.0.0.1:15444 ssl;
                    server_name one.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    return "selected|$ssl_server_name|$server_name";
                }}
                server {{
                    listen 127.0.0.1:15444 ssl;
                    server_name *.wild.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    return "wildcard|$ssl_server_name|$server_name";
                }}
                server {{
                    listen 127.0.0.1:15444 ssl;
                    server_name ~^api[0-9]+\.test$;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    return "regex|$ssl_server_name|$server_name";
                }}
            }}
        "#,
        default_certificate.display(),
        default_key.display(),
        alternate_certificate.display(),
        alternate_key.display(),
        default_certificate.display(),
        default_key.display(),
        default_certificate.display(),
        default_key.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let alternate_der = load_certificates(&alternate_certificate).unwrap()[0]
        .as_ref()
        .to_vec();
    let default_der = load_certificates(&default_certificate).unwrap()[0]
        .as_ref()
        .to_vec();

    let selected = stream_tls_response(&config, "one.test").await;
    assert_eq!(selected.certificate, alternate_der);
    assert_eq!(selected.body, b"selected|one.test|one.test");

    let wildcard = stream_tls_response(&config, "www.wild.test").await;
    assert_eq!(wildcard.certificate, default_der);
    assert_eq!(wildcard.body, b"wildcard|www.wild.test|*.wild.test");

    let regex = stream_tls_response(&config, "api42.test").await;
    assert_eq!(regex.certificate, default_der);
    assert_eq!(regex.body, b"regex|api42.test|~^api[0-9]+\\.test$");

    let fallback = stream_tls_response(&config, "unknown.test").await;
    assert_eq!(fallback.certificate, default_der);
    assert_eq!(fallback.body, b"default|unknown.test|default.test");
    let _ = std::fs::remove_dir_all(prefix);
}

struct StreamTlsResponse {
    certificate: Vec<u8>,
    body: Vec<u8>,
}

async fn stream_tls_response(config: &Arc<RuntimeConfig>, server_name: &str) -> StreamTlsResponse {
    let route = build_stream_routes(config)[0].clone();
    let local = route.address.clone();
    let state = test_state(config);
    *state.stream_tls_configs.write().await = build_stream_tls_configs(config).unwrap();
    let transport_remote: SocketAddr = "127.0.0.1:45683".parse().unwrap();
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
    let mut client = connector.connect(domain, client_io).await.unwrap();
    let certificate = client.get_ref().1.peer_certificates().unwrap()[0]
        .as_ref()
        .to_vec();
    let mut body = Vec::new();
    if let Err(err) = client.read_to_end(&mut body).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    task.await.unwrap();
    StreamTlsResponse { certificate, body }
}
