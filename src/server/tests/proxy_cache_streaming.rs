use super::support::*;
use super::*;

mod client_abort;

#[tokio::test]
async fn default_buffered_cache_miss_streams_after_the_busy_threshold() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered cache streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let total = 12 * 1024;
    let first = 9 * 1024;
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {total}\r\nConnection: close\r\n\r\n{}",
                    "a".repeat(first)
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all("b".repeat(total - first).as_bytes())
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18725;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45725".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state.clone(),
    ));
    client
        .write_all(b"GET /buffered-cache HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    read_until(&mut client, &mut response, &vec![b'a'; 8 * 1024]).await;
    assert!(
        durable_files(&prefix.join("cache")).is_empty(),
        "cache entry must not be visible while the response is still streaming"
    );

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered cache response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.contains("X-Cache-Status: MISS\r\n"), "{headers}");
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));
    server_task.await.unwrap();
    assert_eq!(
        durable_files(&prefix.join("cache")).len(),
        1,
        "completed buffered response should publish one cache entry"
    );

    let cached = request_body_with_state(
        &config,
        state,
        "GET /buffered-cache HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(cached.as_bytes());
    assert!(headers.contains("X-Cache-Status: HIT\r\n"), "{headers}");
    assert_eq!(body.len(), total);

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_cache_miss_and_publishes_entry_after_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping streaming proxy cache test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  6\r\nfirst\n\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\n\r\n")
            .await
            .unwrap();
        if tokio::time::timeout(Duration::from_millis(300), listener.accept())
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
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18720;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45720".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state.clone(),
    ));
    client
        .write_all(b"GET /streamed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    read_until(&mut client, &mut response, b"first\n").await;
    assert!(
        durable_files(&prefix.join("cache")).is_empty(),
        "cache entry must not be visible before upstream EOF"
    );

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming cache response should complete")
        .unwrap();
    let first = String::from_utf8(response).unwrap();
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(
        first.find("first\n").unwrap() < first.find("second\n").unwrap(),
        "{first}"
    );
    server_task.await.unwrap();
    assert_eq!(
        durable_files(&prefix.join("cache")).len(),
        1,
        "completed streaming response should publish one cache entry"
    );

    let second = request_body_with_state(
        &config,
        state,
        "GET /streamed HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("first\nsecond\n"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn truncated_stream_does_not_publish_cache_entry() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping truncated cache test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nfirst")
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18722;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let _ = request_bytes_with_state(
        &config,
        test_state(&config),
        "GET /truncated HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    backend.await.unwrap();
    wait_for_files(&prefix.join("cache")).await;
    assert!(
        all_files(&prefix.join("cache")).is_empty(),
        "truncated cache streaming response left temporary or published files"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn cache_lock_is_held_until_streaming_response_completes() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping streaming cache lock test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\
                  Connection: close\r\n\r\n5\r\nfirst\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n").await.unwrap();
        if tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_ok()
        {
            panic!("streaming cache lock was released before the response completed");
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
                        listen 127.0.0.1:18723;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_cache proxy_cache;
                            proxy_cache_valid 200 1m;
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
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45723".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let first_server = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state.clone(),
    ));
    let request = "GET /locked HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    client.write_all(request.as_bytes()).await.unwrap();
    let mut first = Vec::new();
    read_until(&mut client, &mut first, b"first").await;

    let second_config = config.clone();
    let second_state = state;
    let second = tokio::spawn(async move {
        request_body_with_state(&second_config, second_state, request).await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !second.is_finished(),
        "concurrent cache miss should wait for the streaming response and cache lock"
    );

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut first))
        .await
        .expect("first streaming cache response should complete")
        .unwrap();
    first_server.await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .expect("second request should complete after the cache lock is released")
        .unwrap();
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(
        second.contains("first") && second.contains("second"),
        "{second}"
    );

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_until<R: AsyncRead + Unpin>(client: &mut R, response: &mut Vec<u8>, needle: &[u8]) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(needle.len())
            .any(|window| window == needle)
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming the cache body");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first cache body chunk should arrive before upstream EOF");
}

fn durable_files(root: &Path) -> Vec<PathBuf> {
    all_files(root)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_none_or(|name| !name.to_string_lossy().starts_with('.'))
        })
        .collect()
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
