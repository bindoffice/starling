use super::support::*;
use super::*;

async fn bind_local_listener() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream retry test: sockets not permitted");
            None
        }
        Err(err) => panic!("bind stream retry listener: {err}"),
    }
}

async fn closed_local_address() -> Option<SocketAddr> {
    let listener = bind_local_listener().await?;
    let address = listener.local_addr().unwrap();
    drop(listener);
    Some(address)
}

async fn run_stream_proxy(config: &Arc<RuntimeConfig>, request: &[u8]) -> Vec<u8> {
    let route = build_stream_routes(config)[0].clone();
    let state = test_state(config);
    let remote: SocketAddr = "127.0.0.1:45681".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let proxy = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });

    client.write_all(request).await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    proxy.await.unwrap();
    response
}

#[test]
fn compiles_and_inherits_stream_next_upstream_controls() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_next_upstream off;
                proxy_next_upstream_tries 3;
                proxy_next_upstream_timeout 2s;
                server {
                    listen 127.0.0.1:13430;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:13431;
                    proxy_pass 127.0.0.1:15460;
                    proxy_next_upstream on;
                    proxy_next_upstream_tries 1;
                    proxy_next_upstream_timeout 500ms;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(!servers[0].proxy_next_upstream);
    assert_eq!(servers[0].proxy_next_upstream_tries, 3);
    assert_eq!(
        servers[0].proxy_next_upstream_timeout,
        Duration::from_secs(2)
    );
    assert!(servers[1].proxy_next_upstream);
    assert_eq!(servers[1].proxy_next_upstream_tries, 1);
    assert_eq!(
        servers[1].proxy_next_upstream_timeout,
        Duration::from_millis(500)
    );
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13432;
                    proxy_pass 127.0.0.1:15460;
                }
            }
        "#,
        &[],
    );
    let server = &config.stream.as_ref().unwrap().servers[0];
    assert!(server.proxy_next_upstream);
    assert_eq!(server.proxy_next_upstream_tries, 0);
    assert_eq!(server.proxy_next_upstream_timeout, Duration::ZERO);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_next_upstream_controls() {
    for (value, expected) in [
        (
            "proxy_next_upstream maybe;",
            "invalid value \"maybe\" in \"proxy_next_upstream\" directive",
        ),
        (
            "proxy_next_upstream_tries -1;",
            "invalid value \"-1\" in \"proxy_next_upstream_tries\" directive",
        ),
        (
            "proxy_next_upstream_timeout invalid;",
            "invalid time value \"invalid\"",
        ),
    ] {
        let error = test_config_error(&format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:13433;
                        proxy_pass 127.0.0.1:15460;
                        {value}
                    }}
                }}
            "#
        ));
        assert!(error.contains(expected), "unexpected error: {error}");
    }
}

#[tokio::test]
async fn retries_stream_upstream_after_connect_failure() {
    let Some(backend) = bind_local_listener().await else {
        return;
    };
    let backend_address = backend.local_addr().unwrap();
    let Some(dead_address) = closed_local_address().await else {
        return;
    };
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                upstream backend {{
                    server {dead_address};
                    server {backend_address} backup;
                }}
                log_format retry "$upstream_addr|$status";
                server {{
                    listen 127.0.0.1:13434;
                    proxy_pass backend;
                    access_log logs/retry.log retry;
                    proxy_connect_timeout 100ms;
                    proxy_next_upstream on;
                    proxy_next_upstream_tries 2;
                    proxy_next_upstream_timeout 1s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = run_stream_proxy(&config, b"ping").await;
    assert_eq!(response, b"pong");
    backend_task.await.unwrap();
    let access_log = std::fs::read_to_string(prefix.join("logs/retry.log")).unwrap();
    assert_eq!(
        access_log,
        format!("{dead_address}, {backend_address}|200\n")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn respects_stream_next_upstream_stop_conditions() {
    for (index, directive) in [
        "proxy_next_upstream off;",
        "proxy_next_upstream_tries 1;",
        "proxy_next_upstream_timeout 0.000000001s;",
    ]
    .into_iter()
    .enumerate()
    {
        let Some(backend) = bind_local_listener().await else {
            return;
        };
        let backend_address = backend.local_addr().unwrap();
        let Some(dead_address) = closed_local_address().await else {
            return;
        };
        let listen_port = 13435 + index;
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    upstream backend {{
                        server {dead_address};
                        server {backend_address} backup;
                    }}
                    log_format retry "$upstream_addr|$status";
                    server {{
                        listen 127.0.0.1:{listen_port};
                        proxy_pass backend;
                        access_log logs/retry.log retry;
                        proxy_connect_timeout 100ms;
                        {directive}
                    }}
                }}
            "#
        );
        let (prefix, config) = test_config(&source, &[]);

        let response = run_stream_proxy(&config, b"ping").await;
        assert!(response.is_empty());
        assert!(
            tokio::time::timeout(Duration::from_millis(100), backend.accept())
                .await
                .is_err(),
            "backup upstream was connected despite {directive}"
        );
        let access_log = std::fs::read_to_string(prefix.join("logs/retry.log")).unwrap();
        assert_eq!(access_log, format!("{dead_address}|502\n"));
        let _ = std::fs::remove_dir_all(prefix);
    }
}
