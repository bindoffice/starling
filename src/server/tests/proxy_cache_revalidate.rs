use super::support::*;
use super::*;

const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

#[tokio::test]
async fn revalidates_expired_entry_and_refreshes_its_validity() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy cache revalidate test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\
             ETag: \"v1\"\r\nLast-Modified: {LAST_MODIFIED}\r\n\r\ncached"
        );
        stream.write_all(response.as_bytes()).await.unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request_headers(&mut stream).await;
        let request_lower = request.to_ascii_lowercase();
        assert!(
            request_lower.contains("if-none-match: \"v1\"\r\n"),
            "{request}"
        );
        assert!(
            request_lower.contains(&format!(
                "if-modified-since: {}\r\n",
                LAST_MODIFIED.to_ascii_lowercase()
            )),
            "{request}"
        );
        stream
            .write_all(b"HTTP/1.1 304 Not Modified\r\nConnection: close\r\nETag: \"v1\"\r\n\r\n")
            .await
            .unwrap();

        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("revalidated cache entry unexpectedly caused another upstream request");
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
                        listen 127.0.0.1:18621;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                            proxy_cache_revalidate on;
                            proxy_hide_header ETag;
                            proxy_hide_header Last-Modified;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        second.contains("X-Cache-Status: REVALIDATED\r\n"),
        "{second}"
    );
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");

    let third = request_body_with_state(&config, state, request).await;
    assert!(third.contains("X-Cache-Status: HIT\r\n"), "{third}");
    assert!(third.ends_with("cached"), "{third}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn disabled_revalidate_does_not_send_cached_conditionals() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping disabled proxy cache revalidate test: binding TCP sockets is not permitted"
            );
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\
             ETag: \"v1\"\r\nLast-Modified: {LAST_MODIFIED}\r\n\r\nold"
        );
        stream.write_all(response.as_bytes()).await.unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request_headers(&mut stream).await;
        let request_lower = request.to_ascii_lowercase();
        assert!(!request_lower.contains("if-none-match:"), "{request}");
        assert!(!request_lower.contains("if-modified-since:"), "{request}");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfresh")
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18622;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.ends_with("old"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: EXPIRED\r\n"), "{second}");
    assert!(second.ends_with("fresh"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_headers(stream: &mut TcpStream) -> String {
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
