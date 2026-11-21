use super::support::*;
use super::*;

#[tokio::test]
async fn stale_while_revalidate_starts_background_update_without_use_stale() {
    let Some(listener) = bind_upstream("proxy cache stale-while-revalidate test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let (update_started, update_started_rx) = tokio::sync::oneshot::channel();
    let (release_update, release_update_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(
            &mut stream,
            "Cache-Control: max-age=1, stale-while-revalidate=60",
            "old",
        )
        .await;

        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        update_started.send(()).unwrap();
        release_update_rx.await.unwrap();
        write_response(
            &mut stream,
            "Cache-Control: max-age=60, stale-while-revalidate=60",
            "new",
        )
        .await;
    });
    let (prefix, config) = test_config(&stale_config(upstream, 18635), &[]);
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
        .expect("stale-while-revalidate response should not wait for the update")
        .unwrap();
    assert!(stale.contains("X-Cache-Status: STALE\r\n"), "{stale}");
    assert!(stale.ends_with("old"), "{stale}");

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
async fn stale_if_error_serves_stale_without_use_stale() {
    let Some(listener) = bind_upstream("proxy cache stale-if-error test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(
            &mut stream,
            "Cache-Control: max-age=1, stale-if-error=60",
            "old",
        )
        .await;
    });
    let (prefix, config) = test_config(&stale_config(upstream, 18636), &[]);
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("old"), "{first}");
    backend.await.unwrap();

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.contains("X-Cache-Status: STALE\r\n"), "{second}");
    assert!(second.ends_with("old"), "{second}");

    let _ = std::fs::remove_dir_all(prefix);
}

fn stale_config(upstream: SocketAddr, port: u16) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                proxy_cache_path cache keys_zone=proxy_cache:1m;
                server {{
                    listen 127.0.0.1:{port};
                    add_header X-Cache-Status $upstream_cache_status always;
                    location / {{
                        proxy_pass http://{upstream};
                        proxy_cache proxy_cache;
                        proxy_cache_valid 200 1s;
                        proxy_cache_background_update on;
                    }}
                }}
            }}
        "#
    )
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

async fn write_response(stream: &mut TcpStream, header: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{header}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
