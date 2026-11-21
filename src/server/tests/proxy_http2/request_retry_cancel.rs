use super::*;

#[cfg(unix)]
#[tokio::test]
async fn unix_client_close_cancels_http2_retry_and_cleans_body_file() {
    let first_path = test_socket_path("h2-retry-cancel-first");
    let second_path = test_socket_path("h2-retry-cancel-second");
    let frontend_path = test_socket_path("h2-retry-cancel-frontend");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let Some(frontend_listener) = test_unix_listener(&frontend_path) else {
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
            panic!("HTTP/2 retry started after the Unix client closed");
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
                        listen unix:{};
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
            frontend_path.display(),
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(accept_unix_loop(
        frontend_listener,
        group,
        state,
        shutdown_rx,
    ));

    let mut client = UnixStream::connect(&frontend_path).await.unwrap();
    client
        .write_all(
            b"POST /cancel HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npartmore",
        )
        .await
        .unwrap();
    request_seen_rx.await.unwrap();
    drop(client);
    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();

    first_backend.await.unwrap();
    second_backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "cancelled HTTP/2 retry attempt leaked its file-backed request body"
    );
    shutdown.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Unix accept loop should stop")
        .unwrap();

    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_file(frontend_path);
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
