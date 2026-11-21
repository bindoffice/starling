use super::*;

#[tokio::test]
async fn multiplexes_streaming_http2_responses_on_one_connection() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut first_respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/one");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(())
            .unwrap();
        let mut first = first_respond.send_response(response, false).unwrap();
        first
            .send_data(Bytes::from_static(b"one-first\n"), false)
            .unwrap();

        let (request, mut second_respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/two");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/plain")
            .body(())
            .unwrap();
        let mut second = second_respond.send_response(response, false).unwrap();
        second
            .send_data(Bytes::from_static(b"two-first\n"), false)
            .unwrap();

        let work = async move {
            wait_for_release.await.unwrap();
            first
                .send_data(Bytes::from_static(b"one-second\n"), true)
                .unwrap();
            second
                .send_data(Bytes::from_static(b"two-second\n"), true)
                .unwrap();
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
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18714;
                    location / {
                        proxy_pass http://backend.test/;
                        proxy_http_version 2;
                        proxy_buffering off;
                        proxy_read_timeout 5s;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = ProxyV2Connection::connect(
        Box::new(client_io),
        http2_test_key(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();

    let first_outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/one".into(),
        headers: vec![("Host".into(), "backend.test".into())],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let (first, _) =
        timed_proxy_v2_exchange(&mut connection, &first_outgoing, proxy, context, 0, true)
            .await
            .unwrap();
    let second_outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/two".into(),
        headers: vec![("Host".into(), "backend.test".into())],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let (second, _) =
        timed_proxy_v2_exchange(&mut connection, &second_outgoing, proxy, context, 0, true)
            .await
            .unwrap();

    let ResponseBody::Stream(mut first) = first.body else {
        panic!("first response should be streamed");
    };
    let ResponseBody::Stream(mut second) = second.body else {
        panic!("second response should be streamed");
    };
    let first_chunk = read_stream_until(&mut first, b"one-first\n").await;
    let second_chunk = read_stream_until(&mut second, b"two-first\n").await;
    assert!(first_chunk.windows(10).any(|data| data == b"one-first\n"));
    assert!(second_chunk.windows(10).any(|data| data == b"two-first\n"));

    release.send(()).unwrap();
    let first_rest = read_stream_to_end(&mut first).await;
    let second_rest = read_stream_to_end(&mut second).await;
    assert!(first_rest.windows(11).any(|data| data == b"one-second\n"));
    assert!(second_rest.windows(11).any(|data| data == b"two-second\n"));
    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_http2_upstream_body_before_upstream_eof() {
    let Some(upstream) = bind_test_listener("HTTP/2 upstream streaming test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/events");

        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "text/event-stream")
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from_static(b"first\n"), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from_static(b"second\n"), true)
                .unwrap();
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
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18713;
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_http_version 2;
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
    let remote: SocketAddr = "127.0.0.1:45713".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET /events HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(6).any(|window| window == b"first\n") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming HTTP/2 data");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first HTTP/2 upstream chunk should reach the client before EOF");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming HTTP/2 response should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.find("first\n").unwrap() < response.find("second\n").unwrap(),
        "{response}"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP server should close after the response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_stream_until(stream: &mut ResponseStream, needle: &[u8]) -> Vec<u8> {
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !received.windows(needle.len()).any(|data| data == needle) {
            let Some(ResponseStreamItem::Data(data)) = stream.next().await.unwrap() else {
                panic!("response stream ended before the expected chunk");
            };
            received.extend_from_slice(&data);
        }
    })
    .await
    .expect("stream chunk should arrive before upstream EOF");
    received
}

async fn read_stream_to_end(stream: &mut ResponseStream) -> Vec<u8> {
    let mut received = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(ResponseStreamItem::Data(data)) = stream.next().await.unwrap() {
            received.extend_from_slice(&data);
        }
    })
    .await
    .expect("response stream should complete");
    received
}
