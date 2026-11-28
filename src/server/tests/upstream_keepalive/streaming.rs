use super::*;

#[tokio::test]
async fn reuses_named_http1_upstream_after_a_streaming_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP/1 streaming keepalive test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind HTTP/1 streaming keepalive upstream: {error}"),
    };
    let upstream_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        for (path, body) in [("/one", "first"), ("/two", "second")] {
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(
                request.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                "{request}"
            );
            reader
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 2;
                    }}
                    server {{
                        listen 127.0.0.1:18573;
                        location / {{
                            proxy_pass http://backend;
                            proxy_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let first = request_body_with_state(
        &config,
        state.clone(),
        "GET /one HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(first.contains("first"), "{first}");

    tokio::time::timeout(Duration::from_secs(1), async {
        while state.upstream_keepalive.len("backend") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("streaming response should return its connection to the keepalive pool");

    let second = request_body_with_state(
        &config,
        state,
        "GET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(second.contains("second"), "{second}");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_named_http1_upstream_after_head_and_not_modified_streaming() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping no-body keepalive test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind no-body keepalive upstream: {error}"),
    };
    let upstream_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        for (request_line, response) in [
            (
                "HEAD /head HTTP/1.1\r\n",
                "HTTP/1.1 200 OK\r\nX-Connection: 1\r\nContent-Length: 26\r\n\r\n",
            ),
            (
                "GET /not-modified HTTP/1.1\r\n",
                "HTTP/1.1 304 Not Modified\r\nX-Connection: 1\r\nContent-Length: 100\r\n\r\n",
            ),
            (
                "GET /after HTTP/1.1\r\n",
                "HTTP/1.1 200 OK\r\nX-Connection: 1\r\nContent-Length: 5\r\n\r\nthird",
            ),
        ] {
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(request.starts_with(request_line), "{request}");
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 1;
                    }}
                    server {{
                        listen 127.0.0.1:18574;
                        location / {{
                            proxy_pass http://backend;
                            proxy_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let head = request_body_with_state(
        &config,
        state.clone(),
        "HEAD /head HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"), "{head}");
    assert!(head.contains("X-Connection: 1\r\n"), "{head}");

    let not_modified = request_body_with_state(
        &config,
        state.clone(),
        "GET /not-modified HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        not_modified.starts_with("HTTP/1.1 304 Not Modified\r\n"),
        "{not_modified}"
    );
    assert!(
        not_modified.contains("X-Connection: 1\r\n"),
        "{not_modified}"
    );

    let after = request_body_with_state(
        &config,
        state,
        "GET /after HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(after.starts_with("HTTP/1.1 200 OK\r\n"), "{after}");
    assert!(after.contains("X-Connection: 1\r\n"), "{after}");
    assert!(after.ends_with("third"), "{after}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reuses_named_http1_upstream_after_a_prefetched_buffered_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered streaming keepalive test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind buffered streaming keepalive upstream: {error}"),
    };
    let upstream_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        for (path, body) in [("/one", "x".repeat(8192)), ("/two", "second".into())] {
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                request.push_str(&line);
                if line == "\r\n" {
                    break;
                }
            }
            assert!(
                request.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                "{request}"
            );
            reader
                .get_mut()
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        server {upstream_address};
                        keepalive 2;
                    }}
                    server {{
                        listen 127.0.0.1:18574;
                        location / {{
                            proxy_pass http://backend;
                            proxy_buffering on;
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_max_temp_file_size 0;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let first = request_body_with_state(
        &config,
        state.clone(),
        "GET /one HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(first.as_bytes());
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), 8192);
    assert!(body.iter().all(|byte| *byte == b'x'));

    tokio::time::timeout(Duration::from_secs(1), async {
        while state.upstream_keepalive.len("backend") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("buffered streaming response should return its connection to the pool");

    let second = request_body_with_state(
        &config,
        state,
        "GET /two HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(second.contains("second"), "{second}");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
