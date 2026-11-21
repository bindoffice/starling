use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn content_length_eof_is_bad_request_and_forwards_only_the_prefix() {
    let Some((listener, upstream)) = bind_backend("content-length EOF").await else {
        return;
    };
    let backend = capture_request_until_close(listener);
    let response = send_raw_request(
        upstream,
        18724,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 6\r\n\
          Connection: close\r\n\r\npart",
        true,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert_eq!(request_body(&request), b"part");
}

#[tokio::test]
async fn invalid_chunk_size_is_bad_request_before_upstream_body() {
    let Some((listener, upstream)) = bind_backend("invalid chunk size").await else {
        return;
    };
    let backend = capture_request_until_close(listener);
    let response = send_raw_request(
        upstream,
        18725,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\nZ\r\n",
        false,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert!(request_body(&request).is_empty());
}

#[tokio::test]
async fn truncated_chunk_body_is_bad_request_and_forwards_only_received_bytes() {
    let Some((listener, upstream)) = bind_backend("truncated chunk body").await else {
        return;
    };
    let backend = capture_request_until_close(listener);
    let response = send_raw_request(
        upstream,
        18726,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n5\r\npart",
        true,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert_eq!(request_body(&request), b"4\r\npart\r\n");
}

#[tokio::test]
async fn invalid_chunk_terminator_is_bad_request_after_the_chunk_prefix() {
    let Some((listener, upstream)) = bind_backend("invalid chunk terminator").await else {
        return;
    };
    let backend = capture_request_until_close(listener);
    let response = send_raw_request(
        upstream,
        18727,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n4\r\npartXX",
        false,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert_eq!(request_body(&request), b"4\r\npart\r\n");
}

#[tokio::test]
async fn unterminated_chunk_trailers_are_bad_request_and_do_not_complete_upstream() {
    let Some((listener, upstream)) = bind_backend("unterminated chunk trailers").await else {
        return;
    };
    let backend = capture_request_and_reply_to_complete_chunks(listener);
    let response = send_raw_request(
        upstream,
        18728,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n0\r\nX-Trace: one\r\n",
        true,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert!(!request_body(&request).ends_with(b"0\r\n\r\n"));
}

#[tokio::test]
async fn invalid_chunk_trailer_name_is_bad_request() {
    let Some((listener, upstream)) = bind_backend("invalid chunk trailer").await else {
        return;
    };
    let backend = capture_request_until_close(listener);
    let response = send_raw_request(
        upstream,
        18729,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n0\r\nBad Name: one\r\n\r\n",
        true,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let request = backend.await.unwrap();
    assert!(!request_body(&request).ends_with(b"0\r\n\r\n"));
}

#[tokio::test]
async fn buffered_request_body_rejects_unterminated_trailers_before_upstream() {
    let Some((listener, upstream)) = bind_backend("buffered unterminated trailers").await else {
        return;
    };
    let response = send_raw_request_buffering(
        upstream,
        18733,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n0\r\nX-Trace: one\r\n",
        true,
        false,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err(),
        "buffered request framing failure must happen before connecting upstream"
    );
}

async fn bind_backend(label: &str) -> Option<(TcpListener, SocketAddr)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping proxy request framing test ({label}): TCP binding is not permitted"
            );
            return None;
        }
        Err(error) => panic!("bind {label} upstream: {error}"),
    };
    let address = listener.local_addr().unwrap();
    Some((listener, address))
}

fn capture_request_until_close(listener: TcpListener) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .expect("upstream request should close after request framing fails")
                .unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        request
    })
}

fn capture_request_and_reply_to_complete_chunks(
    listener: TcpListener,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .expect("upstream request should either finish or close")
                .unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request_body(&request).ends_with(b"0\r\n\r\n") {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
                break;
            }
        }
        request
    })
}

async fn send_raw_request(
    upstream: SocketAddr,
    listen_port: u16,
    request: &[u8],
    close_client_write: bool,
) -> String {
    send_raw_request_buffering(upstream, listen_port, request, close_client_write, true).await
}

async fn send_raw_request_buffering(
    upstream: SocketAddr,
    listen_port: u16,
    request: &[u8],
    close_client_write: bool,
    request_buffering_off: bool,
) -> String {
    let buffering = if request_buffering_off {
        "proxy_request_buffering off;"
    } else {
        ""
    };
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{listen_port};
                        location / {{
                            proxy_pass http://{upstream};
                            {buffering}
                            proxy_send_timeout 5s;
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
    let remote: SocketAddr = format!("127.0.0.1:{}", listen_port + 27_000)
        .parse()
        .unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client.write_all(request).await.unwrap();
    if close_client_write {
        client.shutdown().await.unwrap();
    }

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("request framing error should receive a response")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should close after the request framing error")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
    String::from_utf8(response).unwrap()
}

fn request_body(request: &[u8]) -> &[u8] {
    let position = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("upstream request headers");
    &request[position + 4..]
}
