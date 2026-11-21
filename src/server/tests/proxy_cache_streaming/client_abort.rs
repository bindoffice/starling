use super::*;

#[tokio::test]
async fn client_disconnect_finishes_the_cache_tee_and_publishes_the_entry() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping client abort cache test: TCP binding is not permitted");
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
                    proxy_cache_path cache keys_zone=proxy_cache:1m;
                    server {{
                        listen 127.0.0.1:18743;
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
    let remote: SocketAddr = "127.0.0.1:45743".parse().unwrap();
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

    let mut response = Vec::new();
    read_until(&mut client, &mut response, b"first").await;
    drop(client);
    release.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("disconnected connection should terminate")
        .unwrap();
    backend.await.unwrap();
    assert!(
        wait_for_durable_files(&prefix.join("cache")).await.len() == 1,
        "cache tee should publish the complete entry after the client aborts"
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

async fn wait_for_durable_files(root: &Path) -> Vec<PathBuf> {
    for _ in 0..100 {
        let files = durable_files(root);
        if !files.is_empty() {
            return files;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Vec::new()
}
