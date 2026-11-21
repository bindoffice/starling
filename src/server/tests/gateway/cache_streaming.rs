use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn fastcgi_streaming_response_populates_cache_and_store() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping streaming FastCGI cache/store test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nstreamed-cgi",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        if tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_ok()
        {
            panic!("cache hit unexpectedly reached the FastCGI upstream");
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    fastcgi_cache_path cache keys_zone=fastcgi_cache:1m;
                    server {{
                        listen 127.0.0.1:18721;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            fastcgi_pass {upstream};
                            fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                            fastcgi_buffering off;
                            fastcgi_cache fastcgi_cache;
                            fastcgi_cache_key "$request_method$host$request_uri";
                            fastcgi_cache_valid 200 1m;
                            fastcgi_store on;
                            fastcgi_store_access user:r group:r;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /stored/file HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.contains("streamed-cgi"), "{first}");
    assert_eq!(
        std::fs::read(prefix.join("html/stored/file")).unwrap(),
        b"streamed-cgi"
    );
    assert!(
        std::fs::read_dir(prefix.join("cache"))
            .unwrap()
            .next()
            .is_some(),
        "streaming FastCGI response was not cached"
    );

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.contains("streamed-cgi"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_cache_finishes_after_client_abort() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping FastCGI cache client abort test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        request_seen.send(()).unwrap();
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 11\r\n\r\nfirst",
        )
        .await
        .unwrap();
        release_rx.await.unwrap();
        write_fastcgi_record(&mut stream, FCGI_STDOUT, b"second")
            .await
            .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    fastcgi_cache_path cache keys_zone=fastcgi_cache:1m;
                    server {{
                        listen 127.0.0.1:18754;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            fastcgi_pass {upstream};
                            fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                            fastcgi_buffering off;
                            fastcgi_cache fastcgi_cache;
                            fastcgi_cache_key "$request_method$host$request_uri";
                            fastcgi_cache_valid 200 1m;
                            fastcgi_read_timeout 5s;
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
    let remote: SocketAddr = "127.0.0.1:45721".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state.clone(),
    ));
    client
        .write_all(b"GET /aborted HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("FastCGI upstream should receive the request before the client reset")
        .unwrap();
    let mut response = Vec::new();
    read_until(&mut client, &mut response, b"first").await;
    drop(client);
    release.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after draining the FastCGI cache tee")
        .unwrap();
    backend.await.unwrap();
    let files = wait_for_cache_files(&prefix.join("cache")).await;
    assert_eq!(
        files.len(),
        1,
        "FastCGI cache tee should publish the complete entry after the client aborts"
    );
    let cached = request_body_with_state(
        &config,
        state,
        "GET /aborted HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(cached.as_bytes());
    assert!(headers.contains("X-Cache-Status: HIT\r\n"), "{headers}");
    assert_eq!(body, b"firstsecond");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn wait_for_cache_files(root: &Path) -> Vec<PathBuf> {
    for _ in 0..100 {
        let files = cache_files(root);
        if !files.is_empty() {
            return files;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Vec::new()
}

fn cache_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(cache_files(&path));
        } else if path
            .file_name()
            .is_none_or(|name| !name.to_string_lossy().starts_with('.'))
        {
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
            assert!(read > 0, "starling closed before streaming the cache body");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first cache body chunk should arrive before upstream EOF");
}
