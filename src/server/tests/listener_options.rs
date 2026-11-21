use super::support::*;
use super::*;

#[test]
fn propagates_listen_socket_options_to_http_and_stream_routes() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18310 backlog=2048 rcvbuf=32k reuseport;
                }
            }
            stream {
                server {
                    listen 127.0.0.1:18311 sndbuf=64k so_keepalive=on;
                    return ok;
                }
            }
        "#,
        &[],
    );

    let group = build_listener_groups(&config)[0].clone();
    assert_eq!(group.socket_options.backlog, 2048);
    assert_eq!(group.socket_options.receive_buffer_size, Some(32 * 1024));
    assert!(group.socket_options.reuseport);

    let route = build_stream_routes(&config)[0].clone();
    assert_eq!(route.socket_options.send_buffer_size, Some(64 * 1024));
    assert_eq!(route.socket_options.keepalive, ListenKeepalive::On);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn listener_signature_tracks_socket_option_changes() {
    let (first_prefix, first) = test_config(
        r#"
            events {}
            http { server { listen 127.0.0.1:18312 backlog=10; } }
        "#,
        &[],
    );
    let (second_prefix, second) = test_config(
        r#"
            events {}
            http { server { listen 127.0.0.1:18312 backlog=20; } }
        "#,
        &[],
    );

    assert_ne!(listener_signature(&first), listener_signature(&second));
    let _ = std::fs::remove_dir_all(first_prefix);
    let _ = std::fs::remove_dir_all(second_prefix);
}
