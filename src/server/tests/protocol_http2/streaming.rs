use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn resets_http2_stream_when_body_fails_after_response_commit() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (body_sender, body_receiver) = mpsc::channel(1);
    let mut response = Response::new(200);
    response.set_header("Content-Type", "text/plain");
    response.body = ResponseBody::Stream(ResponseStream::new(body_receiver, None, false));

    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (_, respond) = connection.accept().await.unwrap().unwrap();
        tokio::spawn(async move { while connection.accept().await.is_some() {} });
        send_h2_response(respond, response, "GET", ServerTokens::On, 16 * 1024).await
    });

    let (mut client, connection) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, _) = client.send_request(request, true).unwrap();
    let response = response.await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    body_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    let data = body.data().await.unwrap().unwrap();
    assert_eq!(&data[..], b"first");
    body.flow_control().release_capacity(data.len()).unwrap();

    body_sender
        .send(Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "simulated streaming body truncation",
        )))
        .await
        .unwrap();
    let error = body.data().await.unwrap().unwrap_err();
    assert_eq!(error.reason(), Some(h2::Reason::INTERNAL_ERROR));

    drop(body_sender);
    drop(client);
    driver.abort();
    assert!(
        server
            .await
            .unwrap()
            .is_err_and(|error| error.kind() == io::ErrorKind::UnexpectedEof)
    );
}

#[tokio::test]
async fn client_reset_cancels_a_streaming_body_before_upstream_eof() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (body_sender, body_receiver) = mpsc::channel(1);
    let mut response = Response::new(200);
    response.set_header("Content-Type", "text/plain");
    response.body = ResponseBody::Stream(ResponseStream::new(body_receiver, None, false));

    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (_, respond) = connection.accept().await.unwrap().unwrap();
        tokio::spawn(async move { while connection.accept().await.is_some() {} });
        send_h2_response(respond, response, "GET", ServerTokens::On, 16 * 1024).await
    });

    let (mut client, connection) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, mut request_stream) = client.send_request(request, false).unwrap();
    let response = response.await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    body_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    let data = body.data().await.unwrap().unwrap();
    assert_eq!(&data[..], b"first");
    body.flow_control().release_capacity(data.len()).unwrap();

    request_stream.send_reset(h2::Reason::CANCEL);
    tokio::time::timeout(Duration::from_secs(1), body_sender.closed())
        .await
        .expect("client reset should close the response stream and cancel the producer");
    assert!(
        server.await.unwrap().is_err(),
        "server task should report the downstream reset"
    );

    drop(body);
    drop(client);
    driver.abort();
}

#[tokio::test]
async fn preserve_policy_drains_http2_body_after_client_reset() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (body_sender, body_receiver) = mpsc::channel(1);
    let mut response = Response::new(200);
    response.set_header("Content-Type", "text/plain");
    response.body = ResponseBody::Stream(ResponseStream::new(body_receiver, None, false));
    response.mark_preserve_on_client_abort();

    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (_, respond) = connection.accept().await.unwrap().unwrap();
        tokio::spawn(async move { while connection.accept().await.is_some() {} });
        send_h2_response(respond, response, "GET", ServerTokens::On, 16 * 1024).await
    });

    let (mut client, connection) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, mut request_stream) = client.send_request(request, false).unwrap();
    let response = response.await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();

    body_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    let data = body.data().await.unwrap().unwrap();
    assert_eq!(&data[..], b"first");
    body.flow_control().release_capacity(data.len()).unwrap();

    request_stream.send_reset(h2::Reason::CANCEL);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !body_sender.is_closed(),
        "preserved HTTP/2 response should drain instead of cancelling its producer"
    );
    body_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"second"))))
        .await
        .unwrap();
    drop(body_sender);

    let result = tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("HTTP/2 response task should finish after the drained stream closes")
        .unwrap();
    assert!(
        result.is_err(),
        "client reset should still fail the send task"
    );

    drop(body);
    drop(client);
    driver.abort();
}

#[tokio::test]
async fn http2_ignore_client_abort_keeps_the_upstream_until_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP/2 client abort test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (reset_sent, wait_for_reset) = tokio::sync::oneshot::channel();
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
        wait_for_reset.await.unwrap();

        let mut buffer = [0_u8; 1];
        let premature_close =
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer)).await;
        assert!(
            premature_close.is_err(),
            "HTTP/2 reset cancelled the upstream despite ignore_client_abort: {premature_close:?}"
        );
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\n\r\n")
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("drained HTTP/2 upstream should close after the complete response");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after HTTP/2 drain: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18744 http2;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_ignore_client_abort on;
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
    let remote: SocketAddr = "127.0.0.1:45744".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, mut request_stream) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .unwrap()
        .unwrap();
    let mut body = response.into_body();

    let data = tokio::time::timeout(Duration::from_secs(1), body.data())
        .await
        .expect("the first upstream chunk should reach the HTTP/2 client")
        .unwrap()
        .unwrap();
    assert_eq!(&data[..], b"first\n");
    body.flow_control().release_capacity(data.len()).unwrap();

    request_stream.send_reset(h2::Reason::CANCEL);
    reset_sent.send(()).unwrap();
    backend.await.unwrap();

    drop(body);
    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 connection should close after the client disconnects")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_unbuffered_proxy_body_over_http2_before_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping HTTP/2 streaming test: TCP binding is not permitted");
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
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18711 http2;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
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
    let remote: SocketAddr = "127.0.0.1:45711".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .unwrap()
        .unwrap();
    let mut body = response.into_body();

    let first = tokio::time::timeout(Duration::from_secs(1), async {
        let mut received = Vec::new();
        while !received.windows(6).any(|window| window == b"first\n") {
            let data = body.data().await.unwrap().unwrap();
            body.flow_control().release_capacity(data.len()).unwrap();
            received.extend_from_slice(&data);
        }
        received
    })
    .await
    .expect("the first upstream chunk should reach the HTTP/2 client before EOF");
    assert!(first.windows(6).any(|window| window == b"first\n"));

    release.send(()).unwrap();
    let mut rest = Vec::new();
    loop {
        let next = tokio::time::timeout(Duration::from_secs(2), body.data())
            .await
            .expect("streaming HTTP/2 body should complete");
        let Some(data) = next else {
            break;
        };
        let data = data.unwrap();
        body.flow_control().release_capacity(data.len()).unwrap();
        rest.extend_from_slice(&data);
    }
    assert!(
        rest.windows(7).any(|window| window == b"second\n"),
        "missing second chunk in {:?}",
        String::from_utf8_lossy(&rest)
    );

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should close")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
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
