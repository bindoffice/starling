use super::support::*;
use super::*;
use crate::server::stream::{StreamSocketOptions, connect_stream_tcp};

#[test]
fn compiles_and_inherits_stream_tcp_nodelay() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                tcp_nodelay off;
                server {
                    listen 127.0.0.1:13440;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13441;
                    proxy_pass 127.0.0.1:15460;
                    tcp_nodelay on;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(!servers[0].tcp_nodelay);
    assert!(servers[1].tcp_nodelay);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13442;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
        &[],
    );
    assert!(config.stream.as_ref().unwrap().servers[0].tcp_nodelay);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_tcp_nodelay_values() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                tcp_nodelay maybe;
                server {
                    listen 127.0.0.1:13443;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
    );
    assert!(error.contains("invalid value \"maybe\" in \"tcp_nodelay\" directive"));
}

#[tokio::test]
async fn applies_tcp_nodelay_to_upstream_connections() {
    for (directive, expected) in [("", true), ("tcp_nodelay off;", false)] {
        let Some(backend) = bind_test_listener().await else {
            return;
        };
        let backend_address = backend.local_addr().unwrap();
        let backend_task = tokio::spawn(async move {
            let (mut stream, _) = backend.accept().await.unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_all(b"pong").await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:13444;
                        proxy_pass {backend_address};
                        {directive}
                    }}
                }}
            "#
        );
        let (prefix, config) = test_config(&source, &[]);
        let server = &config.stream.as_ref().unwrap().servers[0];
        let mut upstream = connect_stream_tcp(
            &backend_address.ip().to_string(),
            backend_address.port(),
            None,
            StreamSocketOptions::from_server(server),
        )
        .await
        .unwrap();
        assert_eq!(upstream.nodelay().unwrap(), expected);
        upstream.write_all(b"ping").await.unwrap();
        upstream.shutdown().await.unwrap();
        let mut response = [0_u8; 4];
        upstream.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        backend_task.await.unwrap();
        let _ = std::fs::remove_dir_all(prefix);
    }
}

#[tokio::test]
async fn proxies_stream_traffic_with_tcp_nodelay_disabled() {
    for (directive, expected) in [("", true), ("tcp_nodelay off;", false)] {
        let Some(backend) = bind_test_listener().await else {
            return;
        };
        let backend_address = backend.local_addr().unwrap();
        let backend_task = tokio::spawn(async move {
            let (mut stream, _) = backend.accept().await.unwrap();
            let mut request = [0_u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_all(b"pong").await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:13444;
                        proxy_pass {backend_address};
                        {directive}
                    }}
                }}
            "#
        );

        let (prefix, config) = test_config(&source, &[]);
        assert_eq!(
            config.stream.as_ref().unwrap().servers[0].tcp_nodelay,
            expected
        );
        let route = build_stream_routes(&config)[0].clone();
        let state = test_state(&config);
        let remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        let (mut client, inbound) = tokio::io::duplex(1024);
        let proxy = tokio::spawn(async move {
            handle_stream_connection(inbound, &addresses, route, state).await;
        });

        client.write_all(b"ping").await.unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"pong");
        proxy.await.unwrap();
        backend_task.await.unwrap();
        let _ = std::fs::remove_dir_all(prefix);
    }
}

async fn bind_test_listener() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream tcp_nodelay test: sockets not permitted");
            None
        }
        Err(err) => panic!("bind stream backend: {err}"),
    }
}
