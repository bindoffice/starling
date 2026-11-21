use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_ssl_handshake_timeout() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                ssl_handshake_timeout 2s;
                server {
                    listen 127.0.0.1:15445 ssl;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:15446 ssl;
                    ssl_handshake_timeout 500ms;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].ssl_handshake_timeout, Duration::from_secs(2));
    assert_eq!(servers[1].ssl_handshake_timeout, Duration::from_millis(500));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn closes_stream_tls_handshake_after_configured_timeout() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15447 ssl;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_handshake_timeout 20ms;
                    return ok;
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
    let transport_remote: SocketAddr = "127.0.0.1:45684".parse().unwrap();
    let (_client, inbound) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("stream TLS handshake timeout was not enforced")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
