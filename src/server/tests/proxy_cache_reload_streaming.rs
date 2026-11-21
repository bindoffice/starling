use std::path::{Path, PathBuf};

use super::support::*;
use super::*;
use crate::server::run::reload;

#[tokio::test]
async fn reload_during_cache_and_store_streaming_preserves_the_inflight_fill() {
    let Some(listener) = bind_listener("cache reload streaming upstream").await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let (first_sent, first_received) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_headers(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nfirst")
            .await
            .unwrap();
        stream.flush().await.unwrap();
        let _ = first_sent.send(());
        release_rx.await.unwrap();
        stream.write_all(b"second").await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let (prefix, config) = test_config(&source(upstream, 2000), &[]);
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");
    let request_task = {
        let state = state.clone();
        tokio::spawn(async move {
            request_body_with_state(
                &config,
                state,
                "GET /streamed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            )
            .await
        })
    };

    first_received.await.unwrap();
    std::fs::write(&config_path, source(upstream, 2001)).unwrap();
    reload(&state, &config_path, &prefix).await;
    release_tx.send(()).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(5), request_task)
        .await
        .expect("reloaded cache fill should finish")
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("X-Cache-Status: MISS\r\n"), "{response}");
    assert!(response.ends_with("firstsecond"), "{response}");
    backend.await.unwrap();

    wait_for_path(&prefix.join("html/streamed"), Some(b"firstsecond")).await;
    wait_for_durable_files(&prefix.join("cache")).await;
    let snapshot = state.config_snapshot().await;
    let cached = request_body_with_state(
        &snapshot.config,
        state,
        "GET /streamed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(cached.contains("X-Cache-Status: HIT\r\n"), "{cached}");
    assert!(cached.ends_with("firstsecond"), "{cached}");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

async fn read_headers(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
    }
}

async fn wait_for_path(path: &Path, expected: Option<&[u8]>) {
    for _ in 0..200 {
        match (std::fs::read(path), expected) {
            (Ok(body), Some(expected)) if body == expected => return,
            (Ok(_), None) => return,
            _ => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
    panic!("timed out waiting for {}", path.display());
}

async fn wait_for_durable_files(path: &Path) {
    for _ in 0..200 {
        if !durable_files(path).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for cache entries under {}",
        path.display()
    );
}

fn durable_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(durable_files(&path));
        } else if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".starling-cache-"))
        {
            files.push(path);
        }
    }
    files
}

fn source(upstream: SocketAddr, marker: u64) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_max_body_size {marker};
                proxy_cache_path cache keys_zone=proxy_cache:1m;
                server {{
                    listen 127.0.0.1:18725;
                    add_header X-Cache-Status $upstream_cache_status always;
                    location / {{
                        proxy_pass http://{upstream};
                        proxy_cache proxy_cache;
                        proxy_cache_valid 200 1m;
                        proxy_cache_lock on;
                        proxy_cache_lock_timeout 2s;
                        proxy_store on;
                    }}
                }}
            }}
        "#
    )
}
