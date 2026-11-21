use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_ssl_alpn() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                ssl_alpn h2 http/1.1;
                server {
                    listen 127.0.0.1:15450 ssl;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:15451 ssl;
                    ssl_alpn postgres;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].ssl_alpn, ["h2", "http/1.1"]);
    assert_eq!(servers[1].ssl_alpn, ["postgres"]);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_ssl_alpn_directives() {
    let duplicate = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                ssl_alpn h2;
                ssl_alpn http/1.1;
                server {
                    listen 127.0.0.1:15452 ssl;
                    return ok;
                }
            }
        "#,
    );
    assert!(duplicate.contains("\"ssl_alpn\" directive is duplicate"));

    let too_long = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                ssl_alpn {};
                server {{
                    listen 127.0.0.1:15453 ssl;
                    return ok;
                }}
            }}
        "#,
        "x".repeat(256)
    );
    let error = test_config_error(&too_long);
    assert!(error.contains("protocol too long in \"ssl_alpn\" directive"));
}

#[tokio::test]
async fn negotiates_stream_ssl_alpn_and_exposes_the_protocol() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15454 ssl;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_alpn h2 database;
                    return "$ssl_alpn_protocol";
                }}
            }}
        "#,
        fixture.join("test.crt").display(),
        fixture.join("test.key").display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    *state.stream_tls_configs.write().await = build_stream_tls_configs(&config).unwrap();
    let transport_remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let (client_io, inbound) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    let alpn = vec![b"database".to_vec()];
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
        alpn_protocols: &alpn,
    })
    .unwrap();
    let connector = TlsConnector::from(client_config);
    let domain = TlsServerName::try_from("alpn.test".to_string()).unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    assert_eq!(
        client.get_ref().1.alpn_protocol(),
        Some(b"database".as_slice())
    );
    let mut body = Vec::new();
    if let Err(err) = client.read_to_end(&mut body).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    assert_eq!(body, b"database");
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
