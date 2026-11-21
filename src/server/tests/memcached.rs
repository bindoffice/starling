use super::support::*;
use super::*;
use tokio::io::AsyncBufReadExt;

mod streaming;

#[tokio::test]
async fn serves_memcached_values_with_escaped_keys_and_gzip_flag() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = backend.local_addr().unwrap();
    let backend_task = serve_once(
        backend,
        b"get cache%20key%25value\r\n",
        b"VALUE cache%20key%25value 4 11\r\nhello world\r\nEND\r\n",
    );
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream cache {{
                    server {backend_address};
                }}
                server {{
                    listen 127.0.0.1:18144;
                    location /cache/ {{
                        set $memcached_key "cache key%value";
                        memcached_pass cache;
                        memcached_gzip_flag 4;
                        memcached_read_timeout 2s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /cache/item.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Type: text/plain")),
        "{headers}"
    );
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Encoding: gzip")),
        "{headers}"
    );
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Length: 11")),
        "{headers}"
    );
    assert_eq!(body, b"hello world");
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_named_upstream_keepalive_connections() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping memcached keepalive test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind memcached backend: {error}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        for (expected, response) in [
            (
                b"get item\r\n".as_slice(),
                b"VALUE item 0 3\r\none\r\nEND\r\n".as_slice(),
            ),
            (b"get item\r\n".as_slice(), b"END\r\n".as_slice()),
        ] {
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            assert_eq!(line, expected);
            reader.get_mut().write_all(response).await.unwrap();
        }
        drop(reader);
        if tokio::time::timeout(Duration::from_millis(300), backend.accept())
            .await
            .is_ok()
        {
            panic!("memcached keepalive unexpectedly opened a second connection");
        }
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream cache {{
                    server {backend_address};
                    keepalive 2;
                }}
                server {{
                    listen 127.0.0.1:18149;
                    location / {{
                        set $memcached_key item;
                        memcached_pass cache;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    assert!(first.ends_with("one"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 404 Not Found\r\n"), "{second}");

    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn translates_end_to_404_and_serves_head_headers() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = backend.local_addr().unwrap();
    let missing = serve_once(backend, b"get missing\r\n", b"END\r\n");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18145;
                    location / {{
                        set $memcached_key missing;
                        memcached_pass {backend_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{response}"
    );
    assert!(response.contains("404 Not Found"), "{response}");
    missing.await.unwrap();

    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = backend.local_addr().unwrap();
    let head = serve_once(
        backend,
        b"get item\r\n",
        b"VALUE item 0 5\r\nhello\r\nEND\r\n",
    );
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18146;
                    location / {{
                        set $memcached_key item;
                        memcached_pass {backend_address};
                    }}
                }}
            }}
        "#
    );
    let (head_prefix, head_config) = test_config(&source, &[]);
    let response = request_body(
        &head_config,
        "HEAD /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Length: 5")),
        "{headers}"
    );
    assert!(body.is_empty());
    head.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
    let _ = std::fs::remove_dir_all(head_prefix);
}

#[tokio::test]
async fn retries_invalid_responses_against_the_next_upstream() {
    let bad = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let good = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bad_address = bad.local_addr().unwrap();
    let good_address = good.local_addr().unwrap();
    let bad_task = serve_once(bad, b"get item\r\n", b"BROKEN\r\n");
    let good_task = serve_once(good, b"get item\r\n", b"VALUE item 0 2\r\nok\r\nEND\r\n");
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream cache {{
                    server {bad_address};
                    server {good_address};
                }}
                server {{
                    listen 127.0.0.1:18147;
                    location / {{
                        set $memcached_key item;
                        memcached_pass cache;
                        memcached_next_upstream invalid_response;
                        memcached_next_upstream_tries 2;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    bad_task.await.unwrap();
    good_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_unsupported_methods_and_requires_a_memcached_key() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18148;
                    location /method {
                        set $memcached_key item;
                        memcached_pass 127.0.0.1:9;
                    }
                    location /missing-key {
                        memcached_pass 127.0.0.1:9;
                    }
                }
            }
        "#,
        &[],
    );
    let method = request_body(
        &config,
        "POST /method HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        method.starts_with("HTTP/1.1 405 Not Allowed\r\n"),
        "{method}"
    );

    let key = request_body(
        &config,
        "GET /missing-key HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        key.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{key}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn serve_once(
    listener: TcpListener,
    expected: &'static [u8],
    response: &'static [u8],
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await.unwrap();
        assert_eq!(line, expected);
        reader.get_mut().write_all(response).await.unwrap();
        reader.get_mut().shutdown().await.unwrap();
    })
}
