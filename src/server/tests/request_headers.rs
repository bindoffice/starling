use super::support::*;
use super::*;

#[tokio::test]
async fn enforces_configured_request_header_buffer_limits() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18485;
                    client_header_buffer_size 64;
                    large_client_header_buffers 1 128;
                    location / { return 200 "ok\n"; }
                }
            }
        "#,
        &[],
    );

    let long_target = format!(
        "GET /{} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        "a".repeat(140)
    );
    let response = request_body(&config, &long_target).await;
    assert!(
        response.starts_with("HTTP/1.1 414 Request-URI Too Large\r\n"),
        "{response}"
    );

    let long_header = format!(
        "GET / HTTP/1.1\r\nHost: localhost\r\nX-Long: {}\r\nConnection: close\r\n\r\n",
        "b".repeat(130)
    );
    let response = request_body(&config, &long_header).await;
    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{response}"
    );

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_invalid_header_policy_from_the_selected_server() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18486 default_server;
                    server_name default.test;
                    ignore_invalid_headers on;
                    location / {
                        return 200 "bad=$http_x_bad good=$http_x_good\n";
                    }
                }
                server {
                    listen 127.0.0.1:18486;
                    server_name strict.test;
                    ignore_invalid_headers off;
                    location / { return 200 "bad=$http_x_bad\n"; }
                }
            }
        "#,
        &[],
    );

    let lenient = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: default.test\r\nX_Bad: ignored\r\nnot-a-header\r\nX-Good: kept\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(lenient.starts_with("HTTP/1.1 200 OK\r\n"), "{lenient}");
    assert!(lenient.ends_with("bad= good=kept\n"), "{lenient}");

    let strict = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: strict.test\r\nX_Bad: rejected\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(strict.starts_with("HTTP/1.1 200 OK\r\n"), "{strict}");
    assert!(strict.ends_with("bad=rejected\n"), "{strict}");

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn keeps_underscore_headers_when_enabled_despite_invalid_header_policy() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18487;
                    ignore_invalid_headers off;
                    underscores_in_headers on;
                    location / { return 200 "value=$http_x_custom\n"; }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nX_Custom: accepted\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("value=accepted\n"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn derives_the_http2_header_limit_from_large_client_header_buffers() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18488;
                    large_client_header_buffers 8 16k;
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let addresses = ConnectionAddresses::direct(
        ListenAddress::Inet("127.0.0.1:45678".parse().unwrap()),
        group.address.clone(),
    );
    let limits = http2_limits_for_connection(&config, &group, &addresses);
    assert_eq!(limits.max_header_list_size, 8 * 16 * 1024);
    let _ = std::fs::remove_dir_all(prefix);
}
