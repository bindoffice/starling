use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn inherits_stream_tls_and_proxy_tls_options_from_main() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                ssl_certificate {};
                ssl_certificate_key {};
                ssl_protocols TLSv1.2;
                ssl_prefer_server_ciphers on;
                ssl_verify_depth 2;
                proxy_ssl on;
                proxy_ssl_server_name on;
                proxy_ssl_name upstream.test;
                proxy_ssl_session_reuse off;
                proxy_ssl_verify on;
                proxy_ssl_trusted_certificate {};
                proxy_ssl_verify_depth 3;
                proxy_ssl_protocols TLSv1.2;
                server {{
                    listen 127.0.0.1:15467 ssl;
                    proxy_pass 127.0.0.1:15432;
                }}
                server {{
                    listen 127.0.0.1:15468 ssl;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    proxy_ssl off;
                    proxy_ssl_session_reuse on;
                    ssl_verify_depth 4;
                    proxy_pass 127.0.0.1:15432;
                }}
            }}
        "#,
        fixture.join("test.crt").display(),
        fixture.join("test.key").display(),
        fixture.join("ca.crt").display(),
        fixture.join("crl-server.crt").display(),
        fixture.join("crl-server.key").display(),
    );
    let (prefix, config) = test_config(&source, &[]);
    let servers = &config.stream.as_ref().unwrap().servers;
    let inherited = &servers[0];
    assert_eq!(
        inherited.ssl_certificate.as_deref(),
        Some(fixture.join("test.crt").as_path())
    );
    assert_eq!(
        inherited.ssl_certificate_key.as_deref(),
        Some(fixture.join("test.key").as_path())
    );
    assert!(inherited.proxy_ssl);
    assert!(inherited.proxy_ssl_server_name);
    assert_eq!(inherited.proxy_ssl_name.as_deref(), Some("upstream.test"));
    assert!(!inherited.proxy_ssl_session_reuse);
    assert!(inherited.proxy_ssl_verify);
    assert_eq!(
        inherited.proxy_ssl_trusted_certificate.as_deref(),
        Some(fixture.join("ca.crt").as_path())
    );
    assert_eq!(inherited.proxy_ssl_verify_depth, 3);
    assert_eq!(
        inherited.proxy_ssl_protocols,
        TlsProtocols {
            tls12: true,
            tls13: false,
        }
    );
    assert_eq!(inherited.ssl_verify_depth, 2);

    let overridden = &servers[1];
    assert_eq!(
        overridden.ssl_certificate.as_deref(),
        Some(fixture.join("crl-server.crt").as_path())
    );
    assert!(!overridden.proxy_ssl);
    assert!(overridden.proxy_ssl_session_reuse);
    assert_eq!(overridden.ssl_verify_depth, 4);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn serves_stream_tls_with_certificates_inherited_from_main() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certificate = fixture.join("test.crt");
    let key = fixture.join("test.key");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                ssl_certificate {};
                ssl_certificate_key {};
                server {{
                    listen 127.0.0.1:15469 ssl;
                    return inherited;
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    *state.stream_tls_configs.write().await = build_stream_tls_configs(&config).unwrap();
    let transport_remote: SocketAddr = "127.0.0.1:45690".parse().unwrap();
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
    let domain = TlsServerName::try_from("inherited.test".to_string()).unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(
        client.get_ref().1.peer_certificates().unwrap()[0].as_ref(),
        load_certificates(&certificate).unwrap()[0].as_ref()
    );
    let mut body = Vec::new();
    if let Err(err) = client.read_to_end(&mut body).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    assert_eq!(body, b"inherited");
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
