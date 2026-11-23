use super::support::*;
use super::*;

#[tokio::test]
async fn loads_valid_entries_and_removes_invalid_files() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache loader test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached")
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m
                        loader_files=1 loader_sleep=0ms loader_threshold=0ms;
                    server {{
                        listen 127.0.0.1:18617;
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
    let response = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("cached"), "{response}");

    let cache_root = prefix.join("cache");
    let cache_file = find_cache_file(&cache_root).expect("cache entry");
    let invalid = cache_root.join("not-a-cache-entry");
    std::fs::write(&invalid, b"invalid cache file").unwrap();

    let stats = state.proxy_cache.load().await.unwrap();
    assert_eq!(stats.scanned, 2);
    assert_eq!(stats.loaded, 1);
    assert_eq!(stats.loaded_bytes, cache_file.metadata().unwrap().len());
    assert_eq!(stats.removed, 1);
    assert!(!invalid.exists());
    assert!(cache_file.exists());
    assert!(state.proxy_cache.load().await.unwrap().is_empty());

    let restarted = test_state(&config);
    let stats = restarted.proxy_cache.load().await.unwrap();
    assert_eq!(stats.loaded, 1);
    assert_eq!(stats.removed, 0);

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn cached_accel_charset_survives_restart_and_replays_the_original_body() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache charset test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=koi8-r\r\n\
                  Content-Length: 2\r\nX-Accel-Charset: utf-8\r\nConnection: close\r\n\r\n\
                  \xc1\x9c",
            )
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    charset_map koi8-r utf-8 {{
                        C1 D0B0;
                        9C C2B0;
                    }}
                    proxy_cache_path cache keys_zone=proxy_cache:1m loader_sleep=0ms;
                    server {{
                        listen 127.0.0.1:18637;
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
    let first = request_bytes_with_state(
        &config,
        state,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (first_headers, first_body) = split_http_response(&first);
    assert!(
        first_headers.contains("X-Cache-Status: MISS\r\n"),
        "{first_headers}"
    );
    assert!(
        first_headers.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{first_headers}"
    );
    assert_eq!(first_body, [0xd0, 0xb0, 0xc2, 0xb0]);

    backend.await.unwrap();
    let restarted = test_state(&config);
    let stats = restarted.proxy_cache.load().await.unwrap();
    assert_eq!(stats.loaded, 1, "{stats:?}");

    let cached = request_bytes_with_state(
        &config,
        restarted,
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (cached_headers, cached_body) = split_http_response(&cached);
    assert!(
        cached_headers.contains("X-Cache-Status: HIT\r\n"),
        "{cached_headers}"
    );
    assert!(
        cached_headers.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{cached_headers}"
    );
    assert_eq!(cached_body, [0xd0, 0xb0, 0xc2, 0xb0]);

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn removes_stale_starling_temporaries_but_preserves_active_and_unrelated_files() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                proxy_cache_path cache keys_zone=proxy_cache:1m;
            }
        "#,
        &[],
    );
    let cache_root = prefix.join("cache");
    let temp = cache_root.join("temp");
    std::fs::create_dir_all(&temp).unwrap();
    let temporary = temp.join("pending");
    let active = cache_root.join(format!(".starling-cache-{}-1", std::process::id()));
    let stale =
        dead_process_id().map(|owner| cache_root.join(format!(".starling-cache-{owner}-1")));
    std::fs::write(&temporary, b"temporary").unwrap();
    std::fs::write(&active, b"active").unwrap();
    if let Some(stale) = &stale {
        std::fs::write(stale, b"stale").unwrap();
    }

    let state = test_state(&config);
    let stats = state.proxy_cache.load().await.unwrap();
    assert_eq!(stats.removed, u64::from(stale.is_some()));
    assert!(temporary.exists());
    assert!(active.exists());
    if let Some(stale) = stale {
        assert!(!stale.exists());
    }

    let _ = std::fs::remove_dir_all(prefix);
}

fn dead_process_id() -> Option<u32> {
    (2..100_000).find(|pid| {
        *pid != std::process::id() && !crate::server::temp_cleanup::process_is_alive(*pid)
    })
}

#[tokio::test]
async fn keys_zone_capacity_limits_loaded_entries() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache capacity test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\ncached")
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:8192
                        loader_sleep=0ms;
                    server {{
                        listen 127.0.0.1:18628;
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
    let response = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("cached"), "{response}");

    let cache_root = prefix.join("cache");
    let cache_file = find_cache_file(&cache_root).expect("cache entry");
    let contents = std::fs::read(&cache_file).unwrap();
    for index in 1..=199 {
        std::fs::write(cache_root.join(format!("{index:040x}")), &contents).unwrap();
    }

    let stats = state.proxy_cache.load().await.unwrap();
    assert_eq!(stats.scanned, 200);
    assert!(stats.loaded < stats.scanned, "{stats:?}");
    assert!(stats.removed > 0, "{stats:?}");
    assert_eq!(stats.loaded + stats.removed, stats.scanned);

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

fn find_cache_file(root: &Path) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()? {
        let entry = entry.ok()?;
        let file_type = entry.file_type().ok()?;
        if file_type.is_dir() {
            if let Some(path) = find_cache_file(&entry.path()) {
                return Some(path);
            }
        } else if file_type.is_file() && !entry.file_name().to_string_lossy().starts_with('.') {
            return Some(entry.path());
        }
    }
    None
}
