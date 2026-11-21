use super::support::*;
use super::*;

#[tokio::test]
async fn serves_stale_while_updating_the_cache_in_the_background() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy cache background test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (update_started, update_started_rx) = tokio::sync::oneshot::channel();
    let (release_update, release_update_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nold")
            .await
            .unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        update_started.send(()).unwrap();
        release_update_rx.await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nCache-Control: max-age=60\r\nConnection: close\r\n\r\nnew",
            )
            .await
            .unwrap();

        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("background cache update unexpectedly reached the upstream twice");
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
                        listen 127.0.0.1:18623;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1s;
                            proxy_cache_use_stale updating;
                            proxy_cache_background_update on;
                            proxy_cache_lock on;
                            proxy_cache_lock_timeout 2s;
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
    assert!(first.ends_with("old"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let stale_config = config.clone();
    let stale_state = state.clone();
    let stale =
        tokio::spawn(
            async move { request_body_with_state(&stale_config, stale_state, request).await },
        );
    update_started_rx.await.unwrap();
    let stale = tokio::time::timeout(Duration::from_secs(1), stale)
        .await
        .expect("stale response should not wait for the background update")
        .unwrap();
    assert!(stale.contains("X-Cache-Status: STALE\r\n"), "{stale}");
    assert!(stale.ends_with("old"), "{stale}");

    let updating = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        updating.contains("X-Cache-Status: UPDATING\r\n"),
        "{updating}"
    );
    assert!(updating.ends_with("old"), "{updating}");

    release_update.send(()).unwrap();
    backend.await.unwrap();

    let mut last = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last = request_body_with_state(&config, state.clone(), request).await;
        if last.contains("X-Cache-Status: HIT\r\n") {
            break;
        }
    }
    assert!(last.contains("X-Cache-Status: HIT\r\n"), "{last}");
    assert!(last.ends_with("new"), "{last}");

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn failed_background_update_keeps_stale_entry_and_releases_the_lock() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping failed cache background update test: binding TCP sockets is not permitted"
            );
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (update_started, update_started_rx) = tokio::sync::oneshot::channel();
    let (recovery_started, recovery_started_rx) = tokio::sync::oneshot::channel();
    let (release_recovery, release_recovery_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nold")
            .await
            .unwrap();

        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        update_started.send(()).unwrap();
        // A truncated background response must not replace the usable stale entry.
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nn")
            .await
            .unwrap();
        drop(stream);

        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_headers(&mut stream).await;
        recovery_started.send(()).unwrap();
        release_recovery_rx.await.unwrap();
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nCache-Control: max-age=60\r\nConnection: close\r\n\r\nnew",
            )
            .await
            .unwrap();

        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("recovered background cache update reached the upstream twice");
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
                            proxy_cache_valid 200 1s;
                            proxy_cache_use_stale updating error timeout;
                            proxy_cache_background_update on;
                            proxy_cache_lock on;
                            proxy_cache_lock_timeout 2s;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /recover HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("old"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let stale_config = config.clone();
    let stale_state = state.clone();
    let stale =
        tokio::spawn(
            async move { request_body_with_state(&stale_config, stale_state, request).await },
        );
    update_started_rx.await.unwrap();
    let stale = tokio::time::timeout(Duration::from_secs(1), stale)
        .await
        .expect("stale response should not wait for a failing background update")
        .unwrap();
    assert!(stale.contains("X-Cache-Status: STALE\r\n"), "{stale}");
    assert!(stale.ends_with("old"), "{stale}");

    let mut recovery_started_rx = recovery_started_rx;
    let mut recovered = false;
    for _ in 0..100 {
        let response = request_body_with_state(&config, state.clone(), request).await;
        assert!(
            response.contains("X-Cache-Status: STALE\r\n")
                || response.contains("X-Cache-Status: UPDATING\r\n"),
            "failed update must retain stale cache state: {response}"
        );
        assert!(response.ends_with("old"), "{response}");
        if recovery_started_rx.try_recv().is_ok() {
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        recovered,
        "failed background update did not release the cache lock"
    );

    release_recovery.send(()).unwrap();
    backend.await.unwrap();
    let mut last = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last = request_body_with_state(&config, state.clone(), request).await;
        if last.contains("X-Cache-Status: HIT\r\n") {
            break;
        }
    }
    assert!(last.contains("X-Cache-Status: HIT\r\n"), "{last}");
    assert!(last.ends_with("new"), "{last}");
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
