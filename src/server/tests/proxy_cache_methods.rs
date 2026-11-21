use super::support::*;
use super::*;

#[tokio::test]
async fn caches_configured_post_requests() {
    let Some(listener) = bind_upstream("proxy cache POST test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(request.starts_with("POST /item HTTP/1.1\r\n"), "{request}");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached")
            .await
            .unwrap();
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("cached POST unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18624;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_cache_methods POST;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "POST /item HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn post_is_not_cacheable_by_default() {
    let Some(listener) = bind_upstream("proxy cache default methods test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            assert!(request.starts_with("POST /item HTTP/1.1\r\n"), "{request}");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfresh",
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
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18625;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "POST /item HTTP/1.1\r\nHost: localhost\r\nContent-Length: 7\r\nConnection: close\r\n\r\npayload";
    for _ in 0..2 {
        let response = request_body_with_state(&config, state.clone(), request).await;
        assert!(
            response.contains("X-Cache-Status: BYPASS\r\n"),
            "{response}"
        );
        assert!(response.ends_with("fresh"), "{response}");
    }

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn head_requests_use_the_get_cache_entry() {
    let Some(listener) = bind_upstream("proxy cache HEAD test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let body = "0123456789abcdef".repeat(1024);
    let expected_body = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(request.starts_with("GET /item HTTP/1.1\r\n"), "{request}");
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(body.as_bytes()).await.unwrap();
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("HEAD cache hit unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18626;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_busy_buffers_size 4k;
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
        "HEAD /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.contains("X-Cache-Status: MISS\r\n"), "{head}");
    assert!(
        head.contains(&format!("Content-Length: {}\r\n", expected_body.len())),
        "{head}"
    );
    assert!(head.ends_with("\r\n\r\n"), "{head}");

    let get = request_body_with_state(
        &config,
        state,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(get.contains("X-Cache-Status: HIT\r\n"), "{get}");
    assert!(get.ends_with(&expected_body), "{get}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_upstream(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: binding TCP sockets is not permitted");
            None
        }
        Err(error) => panic!("bind upstream: {error}"),
    }
}

async fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8(request).unwrap()
}
