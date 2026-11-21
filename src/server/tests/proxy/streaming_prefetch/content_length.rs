use super::super::super::support::*;
use super::super::super::*;
use super::common::{
    DEFAULT_BUSY_BUFFERS_SIZE, assert_streams_after_threshold, proxy_config, read_request_head,
    response_body_len,
};

#[tokio::test]
async fn streams_default_buffered_content_length_response_after_busy_threshold() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered prefetch test: TCP binding is not permitted");
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
                     Content-Length: {total}\r\nConnection: close\r\n\r\n{}",
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
    });

    let (prefix, config) = proxy_config(18717, &upstream, "");
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45717".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while response_body_len(&response) < DEFAULT_BUSY_BUFFERS_SIZE {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(
                read > 0,
                "starling closed before the buffered prefix arrived"
            );
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("default buffering should send once the busy-buffer threshold is reached");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("prefetched response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers.contains(&format!("Content-Length: {total}\r\n")),
        "{headers}"
    );
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn keeps_small_default_buffered_response_waiting_for_the_full_body() {
    assert_waits_for_remaining_body("", 18718, 45718, 10, 5).await;
}

#[tokio::test]
async fn explicit_busy_buffers_size_streams_after_threshold() {
    assert_streams_after_threshold(
        "proxy_busy_buffers_size 8k;",
        18719,
        45719,
        40 * 1024,
        9 * 1024,
    )
    .await;
}

#[tokio::test]
async fn explicit_max_temp_file_size_streams_after_threshold() {
    assert_streams_after_threshold(
        "proxy_max_temp_file_size 1m;",
        18724,
        45724,
        40 * 1024,
        9 * 1024,
    )
    .await;
}

async fn assert_waits_for_remaining_body(
    location_directives: &str,
    port: u16,
    remote_port: u16,
    total: usize,
    first: usize,
) {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered threshold test: TCP binding is not permitted");
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
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\
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
    });

    let (prefix, config) = proxy_config(port, &upstream, location_directives);
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = format!("127.0.0.1:{remote_port}").parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /buffered HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut byte = [0_u8; 1];
    assert!(
        tokio::time::timeout(Duration::from_millis(150), client.read(&mut byte))
            .await
            .is_err(),
        "buffered proxy response reached the client before the full body"
    );

    release.send(()).unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
