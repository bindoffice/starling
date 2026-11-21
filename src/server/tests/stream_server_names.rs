use super::support::*;

#[test]
fn compiles_stream_server_name_variants() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13456;
                    server_name exact.test alias.test;
                    return exact;
                }
                server {
                    listen 127.0.0.1:13456;
                    server_name *.wild.test;
                    return wildcard;
                }
                server {
                    listen 127.0.0.1:13456;
                    server_name ~^api[0-9]+\.test$;
                    return regex;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].server_name, "exact.test");
    assert_eq!(servers[0].names.len(), 2);
    assert!(
        servers[0]
            .names
            .iter()
            .any(|name| name.matches("alias.test"))
    );
    assert!(
        servers[1]
            .names
            .iter()
            .any(|name| name.matches("www.wild.test"))
    );
    assert!(
        servers[2]
            .names
            .iter()
            .any(|name| name.matches("api42.test"))
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn routes_ssl_preread_sni_to_stream_virtual_servers() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13456;
                    server_name fallback.test;
                    ssl_preread on;
                    return fallback;
                }
                server {
                    listen 127.0.0.1:13456 default_server;
                    server_name exact.test alias.test;
                    ssl_preread on;
                    return exact;
                }
                server {
                    listen 127.0.0.1:13456;
                    server_name *.wild.test;
                    return wildcard;
                }
                server {
                    listen 127.0.0.1:13456;
                    server_name ~^api[0-9]+\.test$;
                    return regex;
                }
            }
        "#,
        &[],
    );

    for (host, expected) in [
        ("fallback.test", "fallback"),
        ("alias.test", "exact"),
        ("www.wild.test", "wildcard"),
        ("api42.test", "regex"),
        ("unknown.test", "exact"),
    ] {
        let response = stream_response(&config, &stream_client_hello(host, &[])).await;
        assert_eq!(response, expected.as_bytes(), "SNI host {host}");
    }
    let _ = std::fs::remove_dir_all(prefix);
}
