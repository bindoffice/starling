use super::*;

#[tokio::test]
async fn serves_byte_ranges_from_buffered_memcached_values() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = backend.local_addr().unwrap();
    let backend_task = serve_once(
        backend,
        b"get range-item\r\n",
        b"VALUE range-item 0 11\r\nhello world\r\nEND\r\n",
    );
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18150;
                    location / {{
                        set $memcached_key range-item;
                        memcached_pass {backend_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nRange: bytes=0-4\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(
        headers.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{headers}"
    );
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Range: bytes 0-4/11")),
        "{headers}"
    );
    assert_eq!(body, b"hello");
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn buffers_memcached_values_before_body_filters() {
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = backend.local_addr().unwrap();
    let backend_task = serve_once(
        backend,
        b"get filter-item\r\n",
        b"VALUE filter-item 0 11\r\nhello world\r\nEND\r\n",
    );
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18151;
                    location / {{
                        set $memcached_key filter-item;
                        memcached_pass {backend_address};
                        sub_filter hello goodbye;
                        sub_filter_types text/plain;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /item.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers
            .lines()
            .any(|line| line.eq_ignore_ascii_case("Content-Length: 13")),
        "{headers}"
    );
    assert_eq!(body, b"goodbye world");
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
