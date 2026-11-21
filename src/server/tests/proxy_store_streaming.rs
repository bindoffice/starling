use super::support::*;
use super::*;

#[tokio::test]
async fn default_buffered_proxy_store_streams_after_the_busy_threshold() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered proxy_store test: TCP binding is not permitted");
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
                    server {{
                        listen 127.0.0.1:18142;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_store on;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45742".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /buffered-store HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    read_until(&mut client, &mut response, &vec![b'a'; 8 * 1024]).await;
    let stored = prefix.join("html/buffered-store");
    assert!(
        !stored.exists(),
        "stored file must not be published before the response completes"
    );

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered store response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), total);
    server_task.await.unwrap();

    let stored_body = std::fs::read(&stored).unwrap();
    assert_eq!(stored_body.len(), total);
    assert!(stored_body[..first].iter().all(|byte| *byte == b'a'));
    assert!(stored_body[first..].iter().all(|byte| *byte == b'b'));

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_proxy_store_body_and_commits_after_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping streaming proxy_store test: TCP binding is not permitted");
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
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  5\r\nfirst\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream.write_all(b"6\r\nsecond\r\n0\r\n\r\n").await.unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18140;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_store on;
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
    let remote: SocketAddr = "127.0.0.1:45740".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET /stored/file HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming the stored body");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first stored body chunk should arrive before upstream EOF");
    let stored = prefix.join("html/stored/file");
    assert!(
        !stored.exists(),
        "stored file must not be published before upstream EOF"
    );

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming store response should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(
        response.find("first").unwrap() < response.find("second").unwrap(),
        "{response}"
    );
    server_task.await.unwrap();
    assert_eq!(std::fs::read(&stored).unwrap(), b"firstsecond");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn client_disconnect_finishes_the_store_tee() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping disconnected proxy_store test: TCP binding is not permitted");
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
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18141;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_store on;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45741".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /disconnect HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before the first stored chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first chunk should reach the client");
    drop(client);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("disconnected connection should terminate")
        .unwrap();
    backend.await.unwrap();
    let stored = prefix.join("html/disconnect");
    assert!(
        wait_for_file(&stored).await,
        "store tee should finish after the downstream aborts"
    );
    assert!(
        std::fs::read(&stored).unwrap() == b"firstsecond",
        "store tee did not persist the complete upstream response"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn replace_failure_does_not_break_the_completed_downstream_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy_store replace-failure test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "stored response".repeat(512);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
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
                    server {{
                        listen 127.0.0.1:18729;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_store on;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let target = prefix.join("html/replace-failure");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("sentinel"), b"keep").unwrap();

    let response = request_bytes(
        &config,
        "GET /replace-failure HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(response_body, expected.as_bytes());
    assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"keep");
    assert!(
        files_in(&prefix.join("html")).iter().all(|path| !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".starling-store-"))),
        "replace failure left a temporary store file"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn files_in(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_in(&path));
        } else {
            files.push(path);
        }
    }
    files
}

async fn read_until<R: AsyncRead + Unpin>(client: &mut R, response: &mut Vec<u8>, needle: &[u8]) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(needle.len())
            .any(|window| window == needle)
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming the stored body");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the stored body prefix should arrive before upstream EOF");
}

async fn wait_for_file(path: &Path) -> bool {
    for _ in 0..100 {
        if path.is_file() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
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
