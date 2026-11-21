use super::*;

#[tokio::test]
async fn h2_body_task_emits_data_before_end_stream() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_secs(1), 64 * 1024);
        let stream = task.stream();

        let first = next_h2_task_chunk(&stream, &mut connection)
            .await
            .expect("first request data");
        assert_eq!(&first[..], b"part");
        first_tx.send(()).unwrap();
        let rest = next_h2_task_chunk(&stream, &mut connection)
            .await
            .expect("second request data");
        assert_eq!(&rest[..], b"remain");
        assert!(next_h2_task_chunk(&stream, &mut connection).await.is_none());
        task.finish().await.unwrap();

        let response = http::Response::builder().status(200).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::from_static(b"ok"), true).unwrap();
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_rx)
        .await
        .expect("first HTTP/2 request chunk should be available before END_STREAM")
        .unwrap();
    body.send_data(Bytes::from_static(b"remain"), true).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut response_body = response.into_body();
    let data = response_body.data().await.unwrap().unwrap();
    assert_eq!(&data[..], b"ok");

    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

#[tokio::test]
async fn h2_body_task_preserves_413_after_the_accepted_prefix() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_secs(1), 5);
        let stream = task.stream();

        let first = next_h2_task_result(&stream, &mut connection)
            .await
            .unwrap()
            .expect("accepted request prefix");
        assert_eq!(&first[..], b"part");
        let error = next_h2_task_result(&stream, &mut connection)
            .await
            .expect_err("the second chunk should exceed the request limit");
        assert_eq!(crate::http::request_body_error_status(&error), Some(413));
        assert!(matches!(stream.next().await, Ok(None)));
        let task_error = task.finish().await.expect_err("body task should fail");
        assert_eq!(
            crate::http::request_body_error_status(&task_error),
            Some(413)
        );

        let response = http::Response::builder().status(413).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::new(), true).unwrap();
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    body.send_data(Bytes::from_static(b"more"), true).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), 413);
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

#[tokio::test]
async fn h2_proxy_request_body_overflow_returns_413_without_retry() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP/2 request body limit test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (first_part_tx, first_part_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let (read_half, _write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let head = read_test_backend_head(&mut reader).await;
        assert!(head.starts_with("POST /upload HTTP/1.1\r\n"), "{head}");

        let mut prefix = [0_u8; 9];
        reader.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"4\r\npart\r\n");
        first_part_tx.send(()).unwrap();

        let mut extra = [0_u8; 1];
        match tokio::time::timeout(Duration::from_secs(2), reader.read(&mut extra)).await {
            Ok(Ok(0)) => {}
            Ok(Err(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) => {}
            Ok(Ok(read)) => panic!("upstream received {read} byte(s) after the accepted prefix"),
            Ok(Err(error)) => panic!("read upstream request body tail: {error}"),
            Err(_) => panic!("upstream connection remained open after request body overflow"),
        }
    });

    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18462 http2;
                    client_max_body_size 5;
                    location / {{
                        proxy_pass http://{upstream_address}/;
                        proxy_http_version 1.1;
                        proxy_request_buffering off;
                        proxy_connect_timeout 1s;
                        proxy_read_timeout 1s;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45684".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, driver) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    first_part_rx.await.unwrap();
    body.send_data(Bytes::from_static(b"more"), true).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(3), response)
        .await
        .expect("HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), 413);
    backend.await.unwrap();

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn h2_body_task_finishes_when_the_client_resets_the_stream() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (first_tx, first_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, _respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_secs(5), 64 * 1024);
        let stream = task.stream();

        let first = next_h2_task_result(&stream, &mut connection)
            .await
            .unwrap()
            .expect("request prefix");
        assert_eq!(&first[..], b"part");
        first_tx.send(()).unwrap();
        let error = next_h2_task_result(&stream, &mut connection)
            .await
            .expect_err("client reset should abort the request body");
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        assert!(matches!(stream.next().await, Ok(None)));
        let task_error = task
            .finish()
            .await
            .expect_err("body task should be canceled");
        assert_eq!(task_error.kind(), io::ErrorKind::ConnectionAborted);
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    first_rx.await.unwrap();
    body.send_reset(h2::Reason::CANCEL);

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("reset response should complete");
    assert!(response.is_err());
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

#[tokio::test]
async fn h2_body_task_rejects_request_trailers_after_data() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_secs(1), 64 * 1024);
        let stream = task.stream();

        let first = next_h2_task_result(&stream, &mut connection)
            .await
            .unwrap()
            .expect("request data");
        assert_eq!(&first[..], b"part");
        let error = next_h2_task_result(&stream, &mut connection)
            .await
            .expect_err("request trailers should be rejected");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(crate::http::request_body_error_status(&error), Some(400));
        assert!(matches!(stream.next().await, Ok(None)));
        let task_error = task.finish().await.expect_err("body task should fail");
        assert_eq!(
            crate::http::request_body_error_status(&task_error),
            Some(400)
        );

        let response = http::Response::builder().status(400).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::new(), true).unwrap();
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-checksum", http::HeaderValue::from_static("done"));
    body.send_trailers(trailers).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), 400);
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

#[tokio::test]
async fn h2_body_timeout_is_classified_as_408() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_millis(20), 64 * 1024);
        let stream = task.stream();
        let mut total = 0_u64;

        let first = next_h2_limited_result(&stream, &mut connection, &mut total, 64 * 1024)
            .await
            .unwrap()
            .expect("request prefix");
        assert_eq!(&first[..], b"part");
        let error = next_h2_limited_result(&stream, &mut connection, &mut total, 64 * 1024)
            .await
            .expect_err("request body should time out");
        assert_eq!(crate::http::request_body_error_status(&error), Some(408));
        assert_eq!(
            task.finish()
                .await
                .expect_err("body task should time out")
                .kind(),
            io::ErrorKind::TimedOut
        );

        let response = http::Response::builder().status(408).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::new(), true).unwrap();
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("timeout response should complete")
        .unwrap();
    assert_eq!(response.status(), 408);
    body.send_reset(h2::Reason::CANCEL);
    drop(sender);
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

async fn next_h2_task_chunk<S>(
    stream: &crate::http::RequestBodyStream,
    connection: &mut h2::server::Connection<S, Bytes>,
) -> Option<Bytes>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let next = stream.next();
    tokio::pin!(next);
    loop {
        tokio::select! {
            item = &mut next => return item.unwrap(),
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "connection closed before request body ended");
            }
        }
    }
}

async fn read_test_backend_head<R>(reader: &mut R) -> String
where
    R: AsyncBufRead + Unpin,
{
    let mut head = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap() == 0 {
            break;
        }
        head.push_str(&line);
        if line == "\r\n" {
            break;
        }
    }
    head
}

async fn next_h2_task_result<S>(
    stream: &crate::http::RequestBodyStream,
    connection: &mut h2::server::Connection<S, Bytes>,
) -> io::Result<Option<Bytes>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let next = stream.next();
    tokio::pin!(next);
    loop {
        tokio::select! {
            item = &mut next => return item,
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "connection closed before request body ended");
            }
        }
    }
}

async fn next_h2_limited_result<S>(
    stream: &crate::http::RequestBodyStream,
    connection: &mut h2::server::Connection<S, Bytes>,
    total: &mut u64,
    limit: u64,
) -> io::Result<Option<Bytes>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let next = stream.next_limited(total, limit);
    tokio::pin!(next);
    loop {
        tokio::select! {
            item = &mut next => return item,
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "connection closed before request body ended");
            }
        }
    }
}
