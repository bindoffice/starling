use super::support::*;
use super::*;

#[tokio::test]
async fn truncated_stream_does_not_publish_store_file() {
    run_failed_store("truncated", 18896, false).await;
}

#[tokio::test]
async fn read_timeout_does_not_publish_store_file() {
    run_failed_store("timeout", 18897, true).await;
}

async fn run_failed_store(label: &str, port: u16, timeout: bool) {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy_store {label} test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 100\r\nConnection: close\r\n\r\nfirst",
            )
            .await
            .unwrap();
        if timeout {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_store on;
                            proxy_read_timeout 100ms;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_bytes(
        &config,
        &format!("GET /store-{label} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
    .await;
    let response_text = String::from_utf8_lossy(&response);
    assert!(
        response_text.starts_with("HTTP/1.1 200 OK\r\n"),
        "{response_text}"
    );
    assert!(response_text.contains("first"), "{response_text}");
    assert!(
        !response_text.contains("502 Bad Gateway"),
        "{response_text}"
    );
    assert!(
        !response_text.contains("504 Gateway Time-out"),
        "{response_text}"
    );

    backend.await.unwrap();
    wait_for_files(&prefix.join("html")).await;
    assert!(
        all_files(&prefix.join("html")).is_empty(),
        "failed proxy_store response left a published or temporary file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(all_files(&path));
        } else {
            files.push(path);
        }
    }
    files
}

async fn wait_for_files(root: &Path) {
    for _ in 0..50 {
        if all_files(root).is_empty() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn read_request_head(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
    }
}
