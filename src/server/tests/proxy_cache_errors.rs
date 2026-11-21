use super::support::*;
use super::*;

#[tokio::test]
async fn caches_negative_proxy_results_until_cache_validity_expires() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping negative proxy cache test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("reserve upstream address: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    drop(listener);

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
                            proxy_cache_valid 502 1ms;
                            proxy_next_upstream off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /unavailable HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{first}");
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");

    let second = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        second.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{second}"
    );
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");

    tokio::time::sleep(Duration::from_millis(10)).await;
    let listener = TcpListener::bind(upstream)
        .await
        .unwrap_or_else(|error| panic!("rebind upstream: {error}"));
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let recovered = request_body_with_state(&config, state, request).await;
    assert!(recovered.starts_with("HTTP/1.1 200 OK\r\n"), "{recovered}");
    assert!(
        recovered.contains("X-Cache-Status: MISS\r\n"),
        "{recovered}"
    );
    assert!(recovered.ends_with("ok"), "{recovered}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request(stream: &mut TcpStream) {
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
}

#[tokio::test]
async fn intercepted_proxy_error_is_served_from_negative_cache() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping intercepted proxy cache test: binding TCP sockets is not permitted"
            );
            return;
        }
        Err(error) => panic!("bind intercepted proxy cache upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 14\r\nConnection: close\r\n\r\nupstream-error",
            )
            .await
            .unwrap();
        assert_no_second_connection(&listener).await;
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
                        error_page 404 /fallback;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 404 1m;
                            proxy_intercept_errors on;
                            proxy_next_upstream off;
                        }}
                        location = /fallback {{
                            return 200 "fallback-ok";
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
    assert!(first.starts_with("HTTP/1.1 404 Not Found\r\n"), "{first}");
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("fallback-ok"), "{first}");
    assert!(!first.contains("upstream-error"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 404 Not Found\r\n"), "{second}");
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("fallback-ok"), "{second}");
    assert!(!second.contains("upstream-error"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn assert_no_second_connection(listener: &TcpListener) {
    if tokio::time::timeout(Duration::from_millis(300), listener.accept())
        .await
        .is_ok()
    {
        panic!("negative cache unexpectedly reached the upstream");
    }
}
