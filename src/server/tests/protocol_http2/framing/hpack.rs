use super::*;
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn accepts_nginx_test_suite_huffman_literal_headers() {
    let headers = [
        0x42, 0x83, 0xc5, 0x83, 0x7f, 0x46, 0x83, 0x9d, 0x29, 0xaf, 0x44, 0x81, 0x63, 0x41, 0x87,
        0xa0, 0xe4, 0x1d, 0x13, 0x9d, 0x09, 0xff,
    ];
    assert_eq!(request_status(&headers).await, Some(0x88));
}

#[tokio::test]
async fn validates_http2_authority_and_host_headers() {
    let missing_authority = [0x82, 0x86, 0x84];
    assert_eq!(request_status(&missing_authority).await, Some(0x8c));

    let empty_authority = [0x82, 0x86, 0x84, 0x41, 0x00];
    assert_eq!(request_status(&empty_authority).await, Some(0x8c));

    let rights = [
        0x82, 0x86, 0x84, 0x41, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 0x00,
        0x04, b'h', b'o', b's', b't', 0x0a, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
        b'2',
    ];
    assert_eq!(request_status(&rights).await, Some(0x8c));

    let host_fallback = [
        0x82, 0x86, 0x84, 0x00, 0x04, b'h', b'o', b's', b't', 0x09, b'l', b'o', b'c', b'a', b'l',
        b'h', b'o', b's', b't',
    ];
    assert_eq!(request_status(&host_fallback).await, Some(0x88));
}

#[tokio::test]
async fn validates_http2_scheme_and_path() {
    let missing_scheme = [
        0x82, 0x84, 0x41, 0x09, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];
    assert_eq!(request_status(&missing_scheme).await, Some(0x8c));

    let relative_path = [
        0x82, 0x86, 0x04, 0x07, b't', b'1', b'.', b'h', b't', b'm', b'l', 0x41, 0x09, b'l', b'o',
        b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];
    assert_eq!(request_status(&relative_path).await, Some(0x8c));

    let invalid_path_control = [
        0x82, 0x86, 0x04, 0x09, b'/', b't', b'1', b'.', b'h', b't', b'm', b'l', 0x02, 0x41, 0x09,
        b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't',
    ];
    assert_eq!(request_status(&invalid_path_control).await, Some(0x8c));
}

#[tokio::test]
async fn rejects_malformed_http2_header_fields() {
    let mut newline = base_request_headers();
    newline.extend(literal_header(b"x-foo", b"x-bar\r\nreferer:see-this"));
    assert_eq!(request_status(&newline).await, Some(0x8c));

    for name in [b"x:foo".as_slice(), b"x foo", b"foo\x02"] {
        let mut headers = base_request_headers();
        headers.extend(literal_header(name, b"bar"));
        assert_eq!(request_status(&headers).await, Some(0x8c));
    }
}

fn base_request_headers() -> Vec<u8> {
    let mut headers = vec![0x82, 0x86, 0x84, 0x41, 0x09];
    headers.extend_from_slice(b"localhost");
    headers
}

fn literal_header(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut header = vec![0, name.len() as u8];
    header.extend_from_slice(name);
    header.push(value.len() as u8);
    header.extend_from_slice(value);
    header
}

async fn request_status(headers: &[u8]) -> Option<u8> {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18464 http2;
                location / { return 200; }
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

    client
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    client
        .write_all(&frame(FRAME_SETTINGS, 0, 0, &[]))
        .await
        .unwrap();
    client
        .write_all(&frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            1,
            headers,
        ))
        .await
        .unwrap();
    client.flush().await.unwrap();

    let frame = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let frame = read_frame(&mut client).await;
            if frame.0 == FRAME_HEADERS || frame.0 == FRAME_GOAWAY {
                return frame;
            }
        }
    })
    .await
    .ok();

    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
    match frame {
        Some((FRAME_HEADERS, _, 1, payload)) => payload.first().copied(),
        _ => None,
    }
}
