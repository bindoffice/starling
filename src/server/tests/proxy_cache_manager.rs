use super::support::*;
use super::*;

#[tokio::test]
async fn removes_inactive_entries_but_hits_refresh_them() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache manager test: binding TCP sockets is not permitted");
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
        if tokio::time::timeout(Duration::from_millis(500), listener.accept())
            .await
            .is_ok()
        {
            panic!("cache hit unexpectedly reached the upstream");
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m
                        inactive=1s manager_files=10 manager_sleep=0ms;
                    server {{
                        listen 127.0.0.1:18615;
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
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");

    let cache_file = find_cache_file(&prefix.join("cache")).expect("cache entry");
    set_old_modified(&cache_file);
    let hit = request_body_with_state(&config, state.clone(), request).await;
    assert!(hit.contains("X-Cache-Status: HIT\r\n"), "{hit}");
    assert!(
        std::fs::metadata(&cache_file).unwrap().modified().unwrap() > SystemTime::UNIX_EPOCH,
        "cache hit did not refresh the file mtime"
    );
    let retained = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(retained.removed, 0);
    assert!(cache_file.exists());

    set_old_modified(&cache_file);
    let removed = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(removed.scanned, 1);
    assert_eq!(removed.removed, 1);
    assert!(removed.freed > 0);
    assert!(!cache_file.exists());

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn enforces_max_size() {
    let Some((prefix, config, backend)) = fill_single_cache_entry(18616, 1, 0).await else {
        return;
    };
    let state = test_state(&config);
    let response = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("cached"), "{response}");
    let cache_file = find_cache_file(&prefix.join("cache")).expect("cache entry");
    let stats = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(stats.removed, 1);
    assert!(!cache_file.exists());

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn enforces_min_free() {
    let Some((prefix, config, backend)) =
        fill_single_cache_entry(18621, 1024 * 1024 * 1024, u64::MAX).await
    else {
        return;
    };
    let state = test_state(&config);
    let response = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("cached"), "{response}");
    let cache_file = find_cache_file(&prefix.join("cache")).expect("cache entry");
    let stats = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(stats.removed, 1);
    assert!(stats.freed > 0);
    assert!(!cache_file.exists());

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn delete_failure_keeps_the_entry_and_recovers_after_permissions_return() {
    use std::os::unix::fs::PermissionsExt;

    if rustix::process::geteuid() == rustix::process::Uid::ROOT {
        eprintln!("skipping cache manager delete failure test as root");
        return;
    }

    let Some((prefix, config, backend)) = fill_single_cache_entry(18626, 1, 0).await else {
        return;
    };
    let state = test_state(&config);
    let response = request_body_with_state(
        &config,
        state.clone(),
        "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("cached"), "{response}");

    let cache_file = find_cache_file(&prefix.join("cache")).expect("cache entry");
    let parent = cache_file
        .parent()
        .expect("cache entry parent")
        .to_path_buf();
    let original = std::fs::metadata(&parent).unwrap().permissions();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();

    let error = state.proxy_cache.maintain().await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{error}");
    assert!(cache_file.exists(), "failed delete removed the cache entry");

    std::fs::set_permissions(&parent, original).unwrap();
    let stats = state.proxy_cache.maintain().await.unwrap();
    assert_eq!(stats.removed, 1);
    assert!(!cache_file.exists());

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn fill_single_cache_entry(
    port: u16,
    max_size: u64,
    min_free: u64,
) -> Option<(PathBuf, Arc<RuntimeConfig>, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping cache manager size test: binding TCP sockets is not permitted");
            return None;
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
                        max_size={max_size} min_free={min_free}
                        manager_files=10 manager_sleep=0ms;
                    server {{
                        listen 127.0.0.1:{port};
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
    Some((prefix, config, backend))
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

fn set_old_modified(path: &Path) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH))
        .unwrap();
}
