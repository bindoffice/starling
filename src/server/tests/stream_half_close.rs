use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

async fn bind_backend() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream half-close test: binding sockets is not permitted");
            None
        }
        Err(err) => panic!("bind stream half-close backend: {err}"),
    }
}

#[test]
fn compiles_and_inherits_stream_proxy_half_close() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_half_close on;
                server {
                    listen 127.0.0.1:15461;
                    proxy_pass 127.0.0.1:15432;
                }
                server {
                    listen 127.0.0.1:15462;
                    proxy_pass 127.0.0.1:15432;
                    proxy_half_close off;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(servers[0].proxy_half_close);
    assert!(!servers[1].proxy_half_close);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:15463;
                    proxy_pass 127.0.0.1:15432;
                }
            }
        "#,
        &[],
    );
    assert!(!config.stream.as_ref().unwrap().servers[0].proxy_half_close);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_proxy_half_close_values() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:15464;
                    proxy_pass 127.0.0.1:15432;
                    proxy_half_close maybe;
                }
            }
        "#,
    );
    assert!(error.contains("invalid value \"maybe\" in \"proxy_half_close\" directive"));
}

#[tokio::test]
async fn proxy_half_close_preserves_the_reverse_direction() {
    let Some(backend) = bind_backend().await else {
        return;
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15465;
                    proxy_pass {backend_address};
                    proxy_half_close on;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45688".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    client.write_all(b"ping").await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("half-closed stream should retain the reverse direction")
        .unwrap();
    assert_eq!(response, b"pong");
    proxy.await.unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn default_stream_proxy_closes_on_the_first_eof() {
    let Some(backend) = bind_backend().await else {
        return;
    };
    let backend_address = backend.local_addr().unwrap();
    let (eof_seen, wait_eof) = tokio::sync::oneshot::channel();
    let (allow_write, write_allowed) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).await.unwrap();
        assert_eq!(request, b"ping");
        eof_seen.send(()).unwrap();
        write_allowed.await.unwrap();
        let _ = stream.write_all(b"late").await;
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen 127.0.0.1:15466;
                    proxy_pass {backend_address};
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45689".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));

    client.write_all(b"ping").await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("default stream proxy should close after the first EOF")
        .unwrap();
    assert!(response.is_empty());
    proxy.await.unwrap();
    wait_eof.await.unwrap();
    allow_write.send(()).unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
