use super::*;

#[tokio::test]
async fn content_length_mismatch_returns_bad_request() {
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
        .method("POST")
        .uri("http://localhost/")
        .header("content-length", "5")
        .body(())
        .unwrap();
    let (response, mut request_body) = sender.send_request(request, false).unwrap();
    request_body
        .send_data(Bytes::from_static(b"part"), true)
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("content-length mismatch response should complete")
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    drop(sender);
    driver.abort();
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

#[tokio::test]
async fn hpack_encoded_field_is_rejected_even_when_decoded_value_fits() {
    // HPACK encodes '!' with a 10-bit Huffman code. The 90-byte decoded
    // value is under the 96-byte limit, but its 113-byte encoding is over it.
    // Send the block directly so the client encoder cannot choose a raw literal.
    let value = huffman_bang(90);
    assert_eq!(value.len(), 113);

    let mut headers = b"\x82\x86\x84\x41\x09localhost".to_vec();
    headers.extend_from_slice(&[0x00, 0x07]);
    headers.extend_from_slice(b"x-expand");
    headers.push(0x80 | value.len() as u8);
    headers.extend_from_slice(&value);

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

fn huffman_bang(count: usize) -> Vec<u8> {
    const BANG_CODE: u32 = 0x3f8;
    const BANG_BITS: u32 = 10;

    let mut output = Vec::with_capacity((count * BANG_BITS as usize).div_ceil(8));
    let mut bits = 0_u64;
    let mut bit_count = 0_u32;
    for _ in 0..count {
        bits = (bits << BANG_BITS) | u64::from(BANG_CODE);
        bit_count += BANG_BITS;
        while bit_count >= 8 {
            bit_count -= 8;
            output.push((bits >> bit_count) as u8);
            bits &= (1_u64 << bit_count) - 1;
        }
    }
    if bit_count != 0 {
        let padding = 8 - bit_count;
        output.push(((bits << padding) | ((1_u64 << padding) - 1)) as u8);
    }
    output
}
