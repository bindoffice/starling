use super::support::*;
use super::*;

#[tokio::test]
async fn reload_preserves_min_uses_counts_for_unchanged_zones() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache index reload test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached",
                )
                .await
                .unwrap();
        }
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("min_uses did not survive reload");
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
                        listen 127.0.0.1:18627;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_cache_min_uses 2;
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

    state.proxy_cache.reload(config.http.as_ref());

    let second = request_body_with_state(&config, state.clone(), request).await;
    assert!(second.contains("X-Cache-Status: MISS\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");
    let third = request_body_with_state(&config, state, request).await;
    assert!(third.contains("X-Cache-Status: HIT\r\n"), "{third}");
    assert!(third.ends_with("cached"), "{third}");

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
