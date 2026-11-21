use super::support::*;
use super::*;

#[tokio::test]
async fn uses_stale_cache_on_configured_upstream_status() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy cache stale test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for (status, body) in [("200 OK", "fresh"), ("500 Internal Server Error", "failed")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
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
                        listen 127.0.0.1:18618;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                            proxy_cache_use_stale http_500;
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
    assert!(first.ends_with("fresh"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let retained = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(retained.removed, 0);

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.contains("X-Cache-Status: STALE\r\n"), "{second}");
    assert!(second.ends_with("fresh"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uses_stale_cache_when_upstream_connection_fails() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping proxy cache stale error test: binding TCP sockets is not permitted"
            );
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18619;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                            proxy_cache_use_stale error;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first_config = config.clone();
    let first_state = state.clone();
    let first =
        tokio::spawn(
            async move { request_body_with_state(&first_config, first_state, request).await },
        );
    let (mut stream, _) = listener.accept().await.unwrap();
    read_request(&mut stream).await;
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfresh")
        .await
        .unwrap();
    let first = first.await.unwrap();
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    drop(listener);

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.contains("X-Cache-Status: STALE\r\n"), "{second}");
    assert!(second.ends_with("fresh"), "{second}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uses_stale_cache_while_another_request_updates_the_entry() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy cache updating test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (updating_started, updating_started_rx) = tokio::sync::oneshot::channel();
    let (release_update, release_update_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nfresh")
            .await
            .unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        updating_started.send(()).unwrap();
        release_update_rx.await.unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nnew")
            .await
            .unwrap();
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("stale updating response unexpectedly reached the upstream");
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
                        listen 127.0.0.1:18620;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                            proxy_cache_use_stale updating;
                            proxy_cache_lock on;
                            proxy_cache_lock_timeout 2s;
                            proxy_cache_lock_age 1s;
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
    assert!(first.ends_with("fresh"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let update_config = config.clone();
    let update_state = state.clone();
    let update = tokio::spawn(async move {
        request_body_with_state(&update_config, update_state, request).await
    });
    updating_started_rx.await.unwrap();

    let stale = request_body_with_state(&config, state, request).await;
    assert!(stale.contains("X-Cache-Status: UPDATING\r\n"), "{stale}");
    assert!(stale.ends_with("fresh"), "{stale}");

    release_update.send(()).unwrap();
    let update = update.await.unwrap();
    assert!(update.ends_with("new"), "{update}");

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
