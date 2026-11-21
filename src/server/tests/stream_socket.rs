use super::support::*;

#[test]
fn compiles_and_inherits_stream_socket_options() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_socket_keepalive on;
                proxy_socket_rcvbuf 32k;
                server {
                    listen 127.0.0.1:13410;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13411;
                    proxy_pass 127.0.0.1:15460;
                    proxy_socket_keepalive off;
                    proxy_socket_rcvbuf 8k;
                    proxy_socket_sndbuf 64k;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(servers[0].proxy_socket_keepalive);
    assert_eq!(servers[0].proxy_socket_rcvbuf, 32 * 1024);
    assert_eq!(servers[0].proxy_socket_sndbuf, 0);
    assert!(!servers[1].proxy_socket_keepalive);
    assert_eq!(servers[1].proxy_socket_rcvbuf, 8 * 1024);
    assert_eq!(servers[1].proxy_socket_sndbuf, 64 * 1024);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13412;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
        &[],
    );
    let server = &config.stream.as_ref().unwrap().servers[0];
    assert!(!server.proxy_socket_keepalive);
    assert_eq!(server.proxy_socket_rcvbuf, 0);
    assert_eq!(server.proxy_socket_sndbuf, 0);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_socket_buffer_sizes() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13413;
                    proxy_pass 127.0.0.1:15460;
                    proxy_socket_rcvbuf invalid;
                }
            }
        "#,
    );
    assert!(error.contains("proxy_socket_rcvbuf"));
    assert!(error.contains("invalid value"));
}
