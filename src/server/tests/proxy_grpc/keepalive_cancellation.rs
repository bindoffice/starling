use super::support::*;
use super::*;

#[cfg(unix)]
#[tokio::test]
async fn grpc_client_abort_keeps_a_multiplexed_upstream_connection_usable() {
    let upstream_path = test_socket_path("grpc-keepalive-cancel-upstream");
    let frontend_path = test_socket_path("grpc-keepalive-cancel-frontend");
    let Some(upstream) = test_unix_listener(&upstream_path) else {
        return;
    };
    let Some(frontend_listener) = test_unix_listener(&frontend_path) else {
        return;
    };

    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/first");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "application/grpc")
            .body(())
            .unwrap();
        let mut first = respond.send_response(response, false).unwrap();
        first
            .send_data(Bytes::from_static(b"first"), false)
            .unwrap();

        let (request, mut respond) = tokio::select! {
            accepted = connection.accept() => accepted
                .expect("gRPC upstream connection closed after the client aborted")
                .expect("gRPC upstream stream"),
            accepted = upstream.accept() => {
                let _ = accepted;
                panic!("gRPC cancellation should keep the existing upstream connection");
            }
        };
        assert_eq!(request.uri().path(), "/reuse");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "application/grpc")
            .body(())
            .unwrap();
        let mut second = respond.send_response(response, false).unwrap();
        second
            .send_data(Bytes::from_static(b"reused"), false)
            .unwrap();
        let mut trailers = http::HeaderMap::new();
        trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
        second.send_trailers(trailers).unwrap();

        shutdown_grpc_h2(connection).await;
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream grpc_backend {{
                        server unix:{};
                        keepalive 1;
                    }}
                    server {{
                        listen unix:{};
                        location / {{
                            grpc_pass grpc_backend;
                            grpc_read_timeout 2s;
                        }}
                    }}
                }}
            "#,
            upstream_path.display(),
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

    let mut first = UnixStream::connect(&frontend_path).await.unwrap();
    first
        .write_all(
            b"POST /first HTTP/1.1\r\nHost: localhost\r\n\
              Content-Type: application/grpc\r\nTE: trailers\r\n\
              Content-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    read_until(&mut first, b"first").await;
    drop(first);
    tokio::time::sleep(Duration::from_millis(150)).await;

    let mut second = UnixStream::connect(&frontend_path).await.unwrap();
    second
        .write_all(
            b"POST /reuse HTTP/1.1\r\nHost: localhost\r\n\
              Content-Type: application/grpc\r\nTE: trailers\r\n\
              Content-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    second.read_to_end(&mut response).await.unwrap();
    assert!(
        response.windows(6).any(|window| window == b"reused"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    backend.await.unwrap();

    shutdown.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Unix accept loop should stop")
        .unwrap();
    let _ = std::fs::remove_file(upstream_path);
    let _ = std::fs::remove_file(frontend_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn grpc_ignore_client_abort_keeps_waiting_for_upstream_headers() {
    let upstream_path = test_socket_path("grpc-ignore-abort-upstream");
    let frontend_path = test_socket_path("grpc-ignore-abort-frontend");
    let Some(upstream) = test_unix_listener(&upstream_path) else {
        return;
    };
    let Some(frontend_listener) = test_unix_listener(&frontend_path) else {
        return;
    };
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (response_sent, response_sent_rx) = tokio::sync::oneshot::channel();

    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/preserved");
        request_seen.send(()).unwrap();

        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(300)) => {}
            accepted = connection.accept() => {
                match accepted {
                    None => panic!("gRPC upstream connection closed despite ignore_client_abort"),
                    Some(Ok(_)) => panic!("unexpected second gRPC stream"),
                    Some(Err(error)) => panic!("gRPC upstream connection failed: {error}"),
                }
            }
        }

        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "application/grpc")
            .body(())
            .unwrap();
        let mut stream = respond
            .send_response(response, false)
            .expect("grpc_ignore_client_abort reset the pending upstream stream");
        stream
            .send_data(Bytes::from_static(b"preserved"), false)
            .unwrap();
        let mut trailers = http::HeaderMap::new();
        trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
        stream.send_trailers(trailers).unwrap();
        response_sent.send(()).unwrap();

        shutdown_grpc_h2(connection).await;
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream grpc_backend {{
                        server unix:{};
                        keepalive 1;
                    }}
                    server {{
                        listen unix:{};
                        location / {{
                            grpc_pass grpc_backend;
                            grpc_ignore_client_abort on;
                            grpc_read_timeout 2s;
                        }}
                    }}
                }}
            "#,
            upstream_path.display(),
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
            b"POST /preserved HTTP/1.1\r\nHost: localhost\r\n\
              Content-Type: application/grpc\r\nTE: trailers\r\n\
              Content-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    request_seen_rx.await.unwrap();
    drop(client);

    tokio::time::timeout(Duration::from_secs(2), response_sent_rx)
        .await
        .expect("preserved gRPC upstream should finish after the client closed")
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    shutdown.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Unix accept loop should stop")
        .unwrap();
    backend.await.unwrap();

    let _ = std::fs::remove_file(upstream_path);
    let _ = std::fs::remove_file(frontend_path);
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_until<R>(reader: &mut R, needle: &[u8])
where
    R: AsyncRead + Unpin,
{
    let mut received = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut buffer))
            .await
            .expect("timed out waiting for gRPC response")
            .unwrap();
        assert!(read > 0, "connection closed before {needle:?}");
        received.extend_from_slice(&buffer[..read]);
        if received
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return;
        }
    }
}
