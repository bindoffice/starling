use std::path::{Path, PathBuf};

use super::support::*;
use super::*;

const BODY: &[u8] = b"partmore";

#[tokio::test]
async fn file_backed_post_body_feeds_cache_and_store_then_cleans_up() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping file-backed cache/store test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind cache/store upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.starts_with(b"POST /item HTTP/1.1\r\n"),
            "{request:?}"
        );
        assert!(request.ends_with(BODY), "{request:?}");
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
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18910;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_cache_methods POST;
                            proxy_store on;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "POST /item HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");
    assert_eq!(std::fs::read(prefix.join("html/item")).unwrap(), b"cached");
    wait_for_durable_files(&prefix.join("cache"), 1).await;

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");

    backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body leaked after cache/store handling"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rewrite_internal_redirect_preserves_then_cleans_file_backed_body() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18911;
                    location = /start {
                        rewrite ^ /internal last;
                    }
                    location = /internal {
                        internal;
                        add_header X-Request-Body-File $request_body_file always;
                        return 200 "redirected";
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "POST /start HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("redirected"), "{response}");
    let request_body_file = response
        .lines()
        .find_map(|line| line.strip_prefix("X-Request-Body-File: "))
        .expect("internal redirect should expose the file-backed request body");
    assert!(
        request_body_file.contains(".starling-tmp-"),
        "unexpected request body path: {request_body_file}"
    );

    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "internal redirect leaked the request body file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn multi_hop_named_internal_redirect_preserves_file_backed_body() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18912;
                    location = /start {
                        rewrite ^ /middle last;
                    }
                    location = /middle {
                        internal;
                        try_files /missing @finish;
                    }
                    location @finish {
                        add_header X-Request-Body-File $request_body_file always;
                        return 200 "named-internal";
                    }
                }
            }
        "#,
        &[],
    );
    let response = request_body(
        &config,
        "POST /start HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("named-internal"), "{response}");
    let request_body_file = response
        .lines()
        .find_map(|line| line.strip_prefix("X-Request-Body-File: "))
        .expect("named internal redirect should keep the file-backed request body");
    assert!(
        request_body_file.contains(".starling-tmp-"),
        "unexpected request body path: {request_body_file}"
    );

    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "multi-hop named redirect leaked the request body file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn error_page_multi_hop_redirect_retains_file_backed_body_variable() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping error-page request body test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind error-page upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.starts_with(b"POST /middle HTTP/1.1\r\n"),
            "{request:?}"
        );
        assert!(request.ends_with(BODY), "{request:?}");
        stream
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 7\r\n\
                  Connection: close\r\n\r\nmissing",
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
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:18913;
                        error_page 404 = /recover;
                        location = /entry {{
                            rewrite ^ /middle last;
                        }}
                        location = /middle {{
                            internal;
                            proxy_intercept_errors on;
                            proxy_pass http://{upstream};
                            add_header X-Response-Hop middle always;
                        }}
                        location = /recover {{
                            internal;
                            try_files /missing @final;
                        }}
                        location @final {{
                            add_header X-Response-Hop final always;
                            add_header X-Request-Body-File $request_body_file always;
                            return 200 "matrix-fallback";
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let response = request_body(
        &config,
        "POST /entry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("matrix-fallback"), "{response}");
    assert!(
        response.contains("\r\nX-Response-Hop: final\r\n"),
        "response filters should use the final error-page location:\n{response}"
    );
    assert!(
        !response.contains("\r\nX-Response-Hop: middle\r\n"),
        "the triggering location leaked response configuration:\n{response}"
    );
    let request_body_file = response
        .lines()
        .find_map(|line| line.strip_prefix("X-Request-Body-File: "))
        .expect("error-page redirect should retain the file-backed request body");
    assert!(
        request_body_file.contains(".starling-tmp-"),
        "unexpected request body path: {request_body_file}"
    );

    backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "error-page redirect leaked the request body file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    };
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before the complete request body");
        request.extend_from_slice(&buffer[..read]);
    }
    request
}

async fn wait_for_temp_cleanup(path: &Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_for_durable_files(path: &Path, expected: usize) {
    for _ in 0..100 {
        if durable_files(path).len() >= expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for {expected} cache entries under {}",
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
            .is_some_and(|name| name.starts_with(".starling-tmp-"))
        {
            files.push(path);
        }
    }
    files
}
