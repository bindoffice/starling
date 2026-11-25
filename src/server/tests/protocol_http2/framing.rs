use super::*;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

mod corpus;
mod header_limits;
mod hpack;
mod mutations;

const FRAME_HEADERS: u8 = 0x1;
const FRAME_DATA: u8 = 0x0;
const FRAME_PRIORITY: u8 = 0x2;
const FRAME_RST_STREAM: u8 = 0x3;
const FRAME_SETTINGS: u8 = 0x4;
const FRAME_PUSH_PROMISE: u8 = 0x5;
const FRAME_PING: u8 = 0x6;
const FRAME_GOAWAY: u8 = 0x7;
const FRAME_WINDOW_UPDATE: u8 = 0x8;
const FRAME_CONTINUATION: u8 = 0x9;
const FRAME_PRIORITY_UPDATE: u8 = 0x10;
const FLAG_END_STREAM: u8 = 0x1;
const FLAG_END_HEADERS: u8 = 0x4;
const FLAG_ACK: u8 = 0x1;
const FLAG_PADDED: u8 = 0x8;

#[tokio::test]
async fn unread_request_body_is_drained_without_closing_the_connection() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
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
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"unread"), true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("first HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let request = http::Request::builder()
        .uri("http://localhost/")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("second HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn request_trailers_send_connection_goaway_with_protocol_error() {
    let mut frames = frame(
        FRAME_HEADERS,
        FLAG_END_HEADERS,
        1,
        b"\x83\x86\x84\x01\x09localhost",
    );
    frames.extend(frame(FRAME_DATA, 0, 1, b"part"));
    frames.extend(frame(
        FRAME_HEADERS,
        FLAG_END_HEADERS | FLAG_END_STREAM,
        1,
        b"\x00\x0ax-checksum\x04done",
    ));
    assert_goaway(frames, h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn data_on_an_idle_stream_sends_connection_goaway() {
    assert_goaway(frame(FRAME_DATA, 0, 1, b"part"), h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn reset_on_an_idle_stream_sends_connection_goaway() {
    assert_goaway(
        frame(
            FRAME_RST_STREAM,
            0,
            1,
            &u32::from(h2::Reason::CANCEL).to_be_bytes(),
        ),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn headers_on_an_even_client_stream_sends_connection_goaway() {
    assert_goaway(
        frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            2,
            b"\x83\x86\x84\x01\x09localhost",
        ),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn data_before_headers_continuation_sends_connection_goaway() {
    let mut frames = frame(
        FRAME_HEADERS,
        FLAG_END_STREAM,
        1,
        b"\x83\x86\x84\x01\x09localhost",
    );
    frames.extend(frame(FRAME_DATA, 0, 1, b"part"));
    assert_goaway(frames, h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn continuation_on_stream_zero_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_CONTINUATION, FLAG_END_HEADERS, 0, &[]),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn settings_on_a_request_stream_sends_connection_goaway() {
    assert_goaway(frame(FRAME_SETTINGS, 0, 1, &[]), h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn zero_window_update_increment_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_WINDOW_UPDATE, 0, 0, &0_u32.to_be_bytes()),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn data_on_stream_zero_sends_connection_goaway() {
    assert_goaway(frame(FRAME_DATA, 0, 0, b"part"), h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn headers_on_stream_zero_sends_connection_goaway() {
    assert_goaway(
        frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            0,
            b"\x83\x86\x84\x01\x09localhost",
        ),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn priority_on_stream_zero_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_PRIORITY, 0, 0, &[0, 0, 0, 1, 15]),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_reset_stream_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_RST_STREAM, 0, 1, &[0, 0, 8]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_ping_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_PING, 0, 0, &[0; 7]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_priority_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_PRIORITY, 0, 1, &[0; 4]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_window_update_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_WINDOW_UPDATE, 0, 0, &[0; 3]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn malformed_settings_length_sends_frame_size_error() {
    assert_goaway(
        frame(FRAME_SETTINGS, 0, 0, &[0; 5]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_goaway_sends_frame_size_error() {
    assert_goaway(
        frame(FRAME_GOAWAY, 0, 0, &[0; 7]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn short_priority_update_sends_frame_size_error() {
    assert_goaway(
        frame(FRAME_PRIORITY_UPDATE, 0, 0, &[0; 3]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn empty_headers_sends_frame_size_error() {
    assert_goaway(
        frame(FRAME_HEADERS, FLAG_END_HEADERS, 1, &[]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn empty_padded_data_sends_frame_size_error() {
    assert_goaway(
        frame(FRAME_DATA, FLAG_PADDED, 1, &[]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn settings_ack_with_payload_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_SETTINGS, FLAG_ACK, 0, &[0; 6]),
        h2::Reason::FRAME_SIZE_ERROR,
    )
    .await;
}

#[tokio::test]
async fn continuation_for_a_different_stream_sends_connection_goaway() {
    let mut frames = frame(
        FRAME_HEADERS,
        FLAG_END_STREAM,
        1,
        b"\x83\x86\x84\x01\x09localhost",
    );
    frames.extend(frame(FRAME_CONTINUATION, FLAG_END_HEADERS, 3, &[]));
    assert_goaway(frames, h2::Reason::PROTOCOL_ERROR).await;
}

#[tokio::test]
async fn push_promise_from_a_client_sends_connection_goaway() {
    assert_goaway(
        frame(FRAME_PUSH_PROMISE, FLAG_END_HEADERS, 1, &[0, 0, 0, 2]),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn invalid_headers_padding_sends_connection_goaway() {
    assert_goaway(
        frame(
            FRAME_HEADERS,
            FLAG_PADDED | FLAG_END_HEADERS | FLAG_END_STREAM,
            1,
            &[10, 0x82],
        ),
        h2::Reason::PROTOCOL_ERROR,
    )
    .await;
}

#[tokio::test]
async fn invalid_hpack_index_sends_connection_goaway() {
    assert_goaway(
        frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS | FLAG_END_STREAM,
            1,
            &[0x80],
        ),
        h2::Reason::COMPRESSION_ERROR,
    )
    .await;
}

async fn assert_goaway(frames: Vec<u8>, expected: h2::Reason) {
    assert_goaway_with_http("", frames, expected).await;
}

async fn assert_goaway_with_http(http_directives: &str, frames: Vec<u8>, expected: h2::Reason) {
    let source = format!(
        r#"
        worker_processes 1;
        events {{ worker_connections 64; }}
        http {{
            {http_directives}
            server {{
                listen 127.0.0.1:18464 http2;
                location / {{ return 204; }}
            }}
        }}
    "#
    );
    let (prefix, config) = test_config(&source, &[]);
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
    client.write_all(&frames).await.unwrap();
    client.flush().await.unwrap();

    let reason = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (kind, _flags, stream_id, payload) = read_frame(&mut client).await;
            if kind == FRAME_GOAWAY {
                assert_eq!(stream_id, 0, "GOAWAY must be connection-scoped");
                assert!(payload.len() >= 8, "truncated GOAWAY payload");
                return u32::from_be_bytes(payload[4..8].try_into().unwrap());
            }
        }
    })
    .await
    .expect("server should send GOAWAY for the malformed frame sequence");
    assert_eq!(reason, u32::from(expected));

    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop after GOAWAY")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn handshake_expands_the_connection_window_to_the_protocol_maximum() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            http2_body_preread_size 96k;
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

    client
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    client.flush().await.unwrap();

    let increment = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let (kind, _flags, stream_id, payload) = read_frame(&mut client).await;
            if kind == FRAME_WINDOW_UPDATE {
                assert_eq!(stream_id, 0);
                assert_eq!(payload.len(), 4);
                return u32::from_be_bytes(payload.try_into().unwrap());
            }
        }
    })
    .await
    .expect("server should send a connection WINDOW_UPDATE");
    assert_eq!(increment, i32::MAX as u32 - 65_535);

    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 server should stop")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn frame(kind: u8, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
    let length = payload.len() as u32;
    let length = length.to_be_bytes();
    let mut output = Vec::with_capacity(9 + payload.len());
    output.extend_from_slice(&length[1..]);
    output.push(kind);
    output.push(flags);
    output.extend_from_slice(&(stream_id & 0x7fff_ffff).to_be_bytes());
    output.extend_from_slice(payload);
    output
}

async fn read_frame<R>(reader: &mut R) -> (u8, u8, u32, Vec<u8>)
where
    R: AsyncRead + Unpin,
{
    let mut header = [0_u8; 9];
    reader.read_exact(&mut header).await.unwrap();
    let length = ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await.unwrap();
    let stream_id = u32::from_be_bytes(header[5..9].try_into().unwrap()) & 0x7fff_ffff;
    (header[3], header[4], stream_id, payload)
}
