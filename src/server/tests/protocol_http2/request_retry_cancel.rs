use super::*;

#[tokio::test]
async fn rst_stream_during_streaming_request_body_cancels_upstream_without_retry() {
    let Some(first) = bind_test_tcp("HTTP/2 streaming reset upstream").await else {
        return;
    };
    let Some(second) = bind_test_tcp("HTTP/2 streaming reset retry backend").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let (prefix_seen, prefix_seen_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (stream, _) = first.accept().await.unwrap();
        let (read_half, _write_half) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(read_half);
        let head = read_backend_head(&mut reader).await;
        assert!(head.starts_with("POST /cancel HTTP/1.1\r\n"), "{head}");
        let mut prefix = [0_u8; 4];
        reader.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"part");
        prefix_seen.send(()).unwrap();

        let mut extra = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut extra))
            .await
            .expect("HTTP/2 reset should close the streaming upstream request");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "upstream received bytes after the HTTP/2 stream reset: {closed:?}"
        );
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("HTTP/2 reset during a streaming body still started a retry");
        }
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                upstream body_retry {{
                    server {first_address};
                    server {second_address};
                }}
                server {{
                    listen 127.0.0.1:18668 http2;
                    location / {{
                        proxy_pass http://body_retry;
                        proxy_request_buffering off;
                        proxy_next_upstream error timeout non_idempotent;
                        proxy_next_upstream_tries 2;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45687".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/cancel")
        .header("content-length", "8")
        .body(())
        .unwrap();
    let (response, mut request_stream) = sender.send_request(request, false).unwrap();
    request_stream
        .send_data(Bytes::from_static(b"part"), false)
        .unwrap();
    prefix_seen_rx.await.unwrap();
    request_stream.send_reset(h2::Reason::CANCEL);

    assert!(
        tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("reset response future should complete")
            .is_err()
    );

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn rst_stream_cancels_http2_proxy_retry_and_cleans_body_file() {
    let first_path = test_socket_path("h2-frontend-retry-cancel-first");
    let second_path = test_socket_path("h2-frontend-retry-cancel-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (stream, _) = first.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            let mut request_body = request.into_body();
            let mut body = Vec::new();
            while let Some(chunk) = request_body.data().await {
                body.extend_from_slice(&chunk.unwrap());
            }
            assert_eq!(body, b"partmore");
            request_seen.send(()).unwrap();
            release_rx.await.unwrap();
            let response = http::Response::builder().status(500).body(()).unwrap();
            if let Ok(mut stream) = respond.send_response(response, true) {
                let _ = stream.send_data(Bytes::new(), true);
            }
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_h2(connection).await;
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("HTTP/2 client reset still started a proxy retry");
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
                    upstream h2_retry {{
                        server unix:{};
                        server unix:{};
                    }}
                    server {{
                        listen 127.0.0.1:18667 http2;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
            first_path.display(),
            second_path.display(),
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/cancel")
        .header("content-length", "8")
        .body(())
        .unwrap();
    let (response, mut request_stream) = sender.send_request(request, false).unwrap();
    request_stream
        .send_data(Bytes::from_static(b"partmore"), true)
        .unwrap();
    request_seen_rx.await.unwrap();
    request_stream.send_reset(h2::Reason::CANCEL);
    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();

    first_backend.await.unwrap();
    second_backend.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("reset response future should complete")
            .is_err()
    );
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "HTTP/2 reset leaked the file-backed request body"
    );

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
async fn wait_for_temp_cleanup(path: &std::path::Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
async fn shutdown_h2(
    mut connection: h2::server::Connection<impl AsyncRead + AsyncWrite + Unpin, Bytes>,
) {
    connection.graceful_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while connection.accept().await.is_some() {}
    })
    .await;
}

async fn read_backend_head<R>(reader: &mut R) -> String
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    let mut head = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await.unwrap();
        assert!(read != 0, "upstream closed before request headers");
        head.push_str(&line);
        if line == "\r\n" {
            return head;
        }
    }
}
