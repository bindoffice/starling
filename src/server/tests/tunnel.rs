use super::support::*;
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn default_tunnel_target_uses_the_connect_authority_port() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18883;
                    location / {
                        tunnel_pass;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let request = Request {
        method: "CONNECT".into(),
        target: "example.test:8443".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "example.test:8443".into(),
        }],
        body: Vec::new(),
        body_stream: None,
        remote_addr: "127.0.0.1:40000".into(),
        local_addr: "127.0.0.1:18883".into(),
        keep_alive: false,
    };
    let local: std::net::SocketAddr = "127.0.0.1:18883".parse().unwrap();
    let remote: std::net::SocketAddr = "127.0.0.1:40000".parse().unwrap();
    let mut variables = Variables::new(
        &request,
        "example.test",
        server,
        &crate::config::ListenAddress::Inet(local),
        &crate::config::ListenAddress::Inet(remote),
    );
    let dispatch = match build_tunnel_dispatch(
        config.http.as_ref().unwrap(),
        &server.locations[0].context,
        &mut variables,
    ) {
        Ok(dispatch) => dispatch,
        Err(_) => panic!("tunnel dispatch failed"),
    };
    assert_eq!(dispatch.response.status, 200);
    assert_eq!(dispatch.targets[0].host, "example.test");
    assert_eq!(dispatch.targets[0].port, 8443);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_connect_without_tunnel_pass() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18880;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 405 "),
        "unexpected CONNECT response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_connect_with_a_request_body_before_upstream_connect() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18881;
                    location / {
                        tunnel_pass 127.0.0.1:9;
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 400 "),
        "unexpected CONNECT body response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn tunnel_pass_rejects_non_connect_requests() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18882;
                    location / {
                        tunnel_pass 127.0.0.1:9;
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 405 "),
        "unexpected non-CONNECT response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn connect_tunnel_returns_200_ok_and_relays_raw_bytes() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP tunnel test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind tunnel backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 5];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"hello");
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18884;
                    location / {{
                        tunnel_pass {backend_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_bytes(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\nhello",
    )
    .await;
    let (head, body) = split_http_response(&response);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, b"world");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn server_level_tunnel_pass_inherits_named_upstream_failover_and_half_close() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping inherited HTTP tunnel test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind tunnel backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 5];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"hello");
        let mut extra = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut extra))
            .await
            .expect("client half-close should reach the tunnel backend");
        assert_eq!(read.unwrap(), 0);
        stream.write_all(b"world").await.unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream tunnel_backend {{
                    server 127.0.0.1:1;
                    server {backend_address};
                }}
                server {{
                    listen 127.0.0.1:18886;
                    tunnel_connect_timeout 100ms;
                    tunnel_next_upstream error;
                    tunnel_next_upstream_tries 2;
                    tunnel_pass tunnel_backend;
                    location / {{}}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_bytes(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\nhello",
    )
    .await;
    let (head, body) = split_http_response(&response);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert_eq!(body, b"world");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn tunnel_read_timeout_closes_after_the_200_response_is_committed() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP tunnel timeout test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind tunnel timeout backend: {error}"),
    };
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1];
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut request))
            .await
            .expect("empty CONNECT payload should half-close the backend");
        assert_eq!(read.unwrap(), 0);
        tokio::time::sleep(Duration::from_millis(200)).await;
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream tunnel_backend {{
                    server {backend_address};
                }}
                server {{
                    listen 127.0.0.1:18887;
                    tunnel_connect_timeout 100ms;
                    tunnel_read_timeout 30ms;
                    tunnel_next_upstream off;
                    tunnel_pass tunnel_backend;
                    location / {{}}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let started = std::time::Instant::now();
    let response = request_bytes(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n",
    )
    .await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "tunnel_read_timeout did not close the committed tunnel promptly"
    );
    let (head, body) = split_http_response(&response);
    assert!(head.starts_with("HTTP/1.1 200 "), "{head}");
    assert!(
        body.is_empty(),
        "timed-out tunnel returned a body: {body:?}"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn maps_tunnel_connect_failure_to_502_before_sending_200() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18885;
                    location / {
                        tunnel_connect_timeout 200ms;
                        tunnel_next_upstream off;
                        tunnel_pass 127.0.0.1:1;
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 502 "),
        "unexpected tunnel failure response:\n{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
