use super::*;

#[tokio::test]
async fn content_length_mismatch_resets_the_request_stream() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            client_body_in_file_only on;
            client_body_temp_path body_temp;
            server {
                listen 127.0.0.1:18464 http2;
                location / { return 204; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let mut headers = b"\x83\x86\x84\x01\x09localhost".to_vec();
    // HPACK static-table entry 28 (content-length), value "5".
    headers.extend_from_slice(b"\x0f\x0d\x015");
    let mut frames = frame(FRAME_HEADERS, FLAG_END_HEADERS, 1, &headers);
    frames.extend(frame(FRAME_DATA, FLAG_END_STREAM, 1, b"part"));
    client
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    client
        .write_all(&frame(FRAME_SETTINGS, 0, 0, &[]))
        .await
        .unwrap();
    client.write_all(&frames).await.unwrap();
    client.flush().await.unwrap();

    let reason = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (kind, _flags, stream_id, payload) = read_frame(&mut client).await;
            if kind == FRAME_RST_STREAM && stream_id == 1 {
                assert_eq!(payload.len(), 4, "truncated RST_STREAM payload");
                return u32::from_be_bytes(payload.try_into().unwrap());
            }
        }
    })
    .await
    .expect("server should reset the stream for the mismatched body");
    assert_eq!(
        reason,
        u32::from(h2::Reason::PROTOCOL_ERROR),
        "content-length mismatch must reset the request stream"
    );

    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop after the bad request")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn oversized_header_list_sends_connection_goaway() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            large_client_header_buffers 4 128;
            server {
                listen 127.0.0.1:18464 http2;
                location / { return 204; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);

    let mut builder = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("x-one", "a".repeat(100));
    for name in ["x-two", "x-three", "x-four", "x-five"] {
        builder = builder.header(name, "b".repeat(100));
    }
    let request = builder.body(()).unwrap();
    let (response, _request_body) = sender.send_request(request, true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("oversized header list should close the request")
        .expect_err("oversized aggregate headers must not produce a normal response");

    drop(sender);
    let error = tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("HTTP/2 client driver should observe the connection error")
        .expect("HTTP/2 client driver task should not panic")
        .expect_err("oversized aggregate headers must close the connection");
    assert_eq!(
        error.reason(),
        Some(h2::Reason::ENHANCE_YOUR_CALM),
        "aggregate header limit should be a connection error: {error:?}"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn oversized_individual_header_field_is_rejected_before_dispatch() {
    let mut headers = b"\x83\x86\x84\x01\x09localhost".to_vec();
    headers.extend_from_slice(b"\x00\x07x-large\x64");
    headers.extend(std::iter::repeat_n(b'a', 100));
    assert_goaway_with_http(
        "large_client_header_buffers 4 96;",
        frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            1,
            &headers,
        ),
        h2::Reason::ENHANCE_YOUR_CALM,
    )
    .await;
}
