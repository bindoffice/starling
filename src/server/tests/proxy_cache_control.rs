use super::support::*;
use super::*;

#[tokio::test]
async fn s_maxage_takes_precedence_over_max_age() {
    let Some(listener) = bind_upstream("proxy cache s-maxage test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_request(&mut stream).await;
        write_response(
            &mut stream,
            "Cache-Control: public, max-age=1, s-maxage=60",
            "cached",
        )
        .await;
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("s-maxage cache entry unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = test_config(&cache_config(upstream, 18632), &[]);
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn invalid_cache_control_disables_storage() {
    let Some(listener) = bind_upstream("proxy cache invalid control test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for body in ["one", "two"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            write_response(&mut stream, "Cache-Control: max-age=broken", body).await;
        }
    });
    let (prefix, config) = test_config(&cache_config(upstream, 18633), &[]);
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("one"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: MISS\r\n"), "{second}");
    assert!(second.ends_with("two"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn accel_buffering_no_disables_proxy_cache_storage() {
    let Some(listener) = bind_upstream("proxy cache accel buffering test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for body in ["one", "two"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            write_response(&mut stream, "X-Accel-Buffering: no", body).await;
        }
    });
    let (prefix, config) = test_config(&cache_config(upstream, 18635), &[]);
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("one"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: MISS\r\n"), "{second}");
    assert!(second.ends_with("two"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn invalid_and_zero_absolute_accel_expiry_follow_nginx_fallback() {
    let Some(listener) = bind_upstream("proxy cache invalid accel expiry test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for (path, expiry) in [("/invalid", "broken"), ("/absolute-zero", "@0")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            assert!(
                request.starts_with(&format!("GET {path} HTTP/1.1\r\n")),
                "{request}"
            );
            write_response(&mut stream, &format!("X-Accel-Expires: {expiry}"), "cached").await;
        }
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("fallback cache entry unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = test_config(&cache_config(upstream, 18634), &[]);
    let state = test_state(&config);

    for path in ["/invalid", "/absolute-zero"] {
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        let first = request_body_with_state(&config, state.clone(), &request).await;
        assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
        assert!(first.ends_with("cached"), "{first}");

        let second = request_body_with_state(&config, state.clone(), &request).await;
        assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
        assert!(second.ends_with("cached"), "{second}");
    }

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn cache_config(upstream: SocketAddr, port: u16) -> String {
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
                        proxy_cache_valid 200 1m;
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

async fn write_response(stream: &mut TcpStream, header: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{header}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
