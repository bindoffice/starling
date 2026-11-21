use super::super::super::support::*;
use super::super::super::*;
use super::common::{
    DEFAULT_BUSY_BUFFERS_SIZE, proxy_config, read_request_head, response_body_len,
};

#[tokio::test]
async fn streams_default_buffered_until_eof_response_after_busy_threshold() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping until-eof streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let total = 12 * 1024;
    let first = 9 * 1024;
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Connection: close\r\n\r\n{}",
                    "a".repeat(first)
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all("b".repeat(total - first).as_bytes())
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });

    let (prefix, config) = proxy_config(18728, &upstream, "");
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45728".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /until-eof HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while response_body_len(&response) < DEFAULT_BUSY_BUFFERS_SIZE {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(
                read > 0,
                "starling closed before the until-EOF buffered prefix arrived"
            );
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("until-EOF default buffering should send once the busy threshold is reached");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("prefetched until-EOF response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    let body = decode_chunked_body(body);
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn decode_chunked_body(mut body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .expect("chunk size line");
        let size = std::str::from_utf8(&body[..line_end])
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        let size = usize::from_str_radix(size.trim(), 16).unwrap();
        body = &body[line_end + 2..];
        if size == 0 {
            return decoded;
        }
        decoded.extend_from_slice(&body[..size]);
        assert_eq!(&body[size..size + 2], b"\r\n");
        body = &body[size + 2..];
    }
}
