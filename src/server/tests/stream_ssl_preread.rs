use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_ssl_preread() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                ssl_preread on;
                preread_timeout 45s;
                preread_buffer_size 32k;
                server {
                    listen 127.0.0.1:13450;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13451;
                    ssl_preread off;
                    preread_timeout 750ms;
                    preread_buffer_size 8k;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(servers[0].ssl_preread);
    assert_eq!(servers[0].preread_timeout, Duration::from_secs(45));
    assert_eq!(servers[0].preread_buffer_size, 32 * 1024);
    assert!(!servers[1].ssl_preread);
    assert_eq!(servers[1].preread_timeout, Duration::from_millis(750));
    assert_eq!(servers[1].preread_buffer_size, 8 * 1024);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_ssl_preread_controls() {
    for (value, expected) in [
        (
            "ssl_preread maybe;",
            "invalid value \"maybe\" in \"ssl_preread\" directive",
        ),
        ("preread_timeout invalid;", "invalid time value \"invalid\""),
        (
            "preread_buffer_size 0;",
            "value must be nonzero in \"preread_buffer_size\" directive",
        ),
    ] {
        let error = test_config_error(&format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:13452;
                        {value}
                        return ok;
                    }}
                }}
            "#
        ));
        assert!(error.contains(expected), "unexpected error: {error}");
    }
}

#[tokio::test]
async fn exposes_ssl_preread_variables() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13453;
                    ssl_preread on;
                    return "$ssl_preread_protocol|$ssl_preread_server_name|$ssl_preread_alpn_protocols\n";
                }
            }
        "#,
        &[],
    );
    let response = stream_response(
        &config,
        &stream_client_hello("Example.Test", &["h2", "http/1.1"]),
    )
    .await;
    assert_eq!(
        String::from_utf8(response).unwrap(),
        "TLSv1.3|example.test|h2,http/1.1\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn routes_stream_by_preread_sni_and_preserves_client_hello() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream ssl_preread proxy test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind stream ssl_preread backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let hello = stream_client_hello("example.test", &["h2"]);
    let expected_hello = hello.clone();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut received = vec![0_u8; expected_hello.len()];
        stream.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected_hello);
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                upstream tls_backend {{
                    server {backend_address};
                }}
                map $ssl_preread_server_name $selected {{
                    example.test tls_backend;
                    default missing;
                }}
                server {{
                    listen 127.0.0.1:13454;
                    ssl_preread on;
                    proxy_pass $selected;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let mut input = hello;
    input.extend_from_slice(b"ping");
    let response = stream_response(&config, &input).await;
    assert_eq!(response, b"pong");
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn times_out_incomplete_ssl_preread() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13455;
                    ssl_preread on;
                    preread_timeout 20ms;
                    return ok;
                }
            }
        "#,
        &[],
    );
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
    let (_client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .expect("ssl preread timeout should close the session")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
