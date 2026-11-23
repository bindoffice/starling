use super::support::*;
use super::*;

#[tokio::test]
async fn caches_ranges_below_the_configured_offset() {
    let Some(listener) = bind_upstream("proxy cache range offset test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(!request.contains("Range:"), "{request}");
        write_response(&mut stream, "cached").await;
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("cached range request unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = range_config(upstream, 18629, 4);
    let state = test_state(&config);

    let ranged = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nRange: bytes=3-\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(ranged.contains("X-Cache-Status: MISS\r\n"), "{ranged}");
    assert!(ranged.ends_with("cached"), "{ranged}");

    let plain = request_body_with_state(
        &config,
        state,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(plain.contains("X-Cache-Status: HIT\r\n"), "{plain}");
    assert!(plain.ends_with("cached"), "{plain}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn does_not_store_suffix_or_ranges_at_the_offset() {
    let Some(listener) = bind_upstream("proxy cache excluded range test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for body in ["above-one", "above-two", "suffix-one", "suffix-two"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            write_response(&mut stream, body).await;
        }
    });
    let (prefix, config) = range_config(upstream, 18630, 4);
    let state = test_state(&config);

    let above = request_body_with_state(
        &config,
        state.clone(),
        "GET /above HTTP/1.1\r\nHost: localhost\r\nRange: bytes=4-\r\nConnection: close\r\n\r\n",
    )
    .await;
    let above_plain = request_body_with_state(
        &config,
        state.clone(),
        "GET /above HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(above.contains("X-Cache-Status: MISS\r\n"), "{above}");
    assert!(above.ends_with("above-one"), "{above}");
    assert!(
        above_plain.contains("X-Cache-Status: MISS\r\n"),
        "{above_plain}"
    );
    assert!(above_plain.ends_with("above-two"), "{above_plain}");

    let suffix = request_body_with_state(
        &config,
        state.clone(),
        "GET /suffix HTTP/1.1\r\nHost: localhost\r\nRange: bytes=-2\r\nConnection: close\r\n\r\n",
    )
    .await;
    let suffix_plain = request_body_with_state(
        &config,
        state,
        "GET /suffix HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(suffix.contains("X-Cache-Status: MISS\r\n"), "{suffix}");
    assert!(suffix.ends_with("suffix-one"), "{suffix}");
    assert!(
        suffix_plain.contains("X-Cache-Status: MISS\r\n"),
        "{suffix_plain}"
    );
    assert!(suffix_plain.ends_with("suffix-two"), "{suffix_plain}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn zero_offset_blocks_range_stores_but_not_cache_hits() {
    let Some(listener) = bind_upstream("proxy cache zero range offset test").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for body in ["cached", "fresh-one", "fresh-two"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut stream).await;
            write_response(&mut stream, body).await;
        }
    });
    let (prefix, config) = range_config(upstream, 18631, 0);
    let state = test_state(&config);

    let cached = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(cached.contains("X-Cache-Status: MISS\r\n"), "{cached}");
    assert!(cached.ends_with("cached"), "{cached}");

    let cached_range = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nRange: bytes=1-\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        cached_range.contains("X-Cache-Status: HIT\r\n"),
        "{cached_range}"
    );
    assert!(cached_range.ends_with("cached"), "{cached_range}");

    let fresh_range = request_body_with_state(
        &config,
        state.clone(),
        "GET /fresh HTTP/1.1\r\nHost: localhost\r\nRange: bytes=1-\r\nConnection: close\r\n\r\n",
    )
    .await;
    let fresh_plain = request_body_with_state(
        &config,
        state,
        "GET /fresh HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        fresh_range.contains("X-Cache-Status: MISS\r\n"),
        "{fresh_range}"
    );
    assert!(fresh_range.ends_with("fresh-one"), "{fresh_range}");
    assert!(
        fresh_plain.contains("X-Cache-Status: MISS\r\n"),
        "{fresh_plain}"
    );
    assert!(fresh_plain.ends_with("fresh-two"), "{fresh_plain}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn range_config(
    upstream: SocketAddr,
    port: u16,
    max_range_offset: u64,
) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
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
                            proxy_cache_max_range_offset {max_range_offset};
                        }}
                    }}
                }}
            "#
        ),
        &[],
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

async fn write_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
}
