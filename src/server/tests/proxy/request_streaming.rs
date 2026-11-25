use super::super::support::*;
use super::super::*;

mod chunk_batching;

#[tokio::test]
async fn merges_duplicate_cookie_headers_for_http1_upstreams() {
    let (mut reader, mut writer) = tokio::io::duplex(64 * 1024);
    let outgoing = ProxyRequestMessage {
        method: "GET".into(),
        path: "/".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Cookie".into(), "a=b".into()),
            ("X-Test".into(), "present".into()),
            ("cookie".into(), "c=d".into()),
        ],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    };
    let task =
        tokio::spawn(async move { write_proxy_request(&mut writer, &outgoing, "1.1").await });

    let mut request = Vec::new();
    reader.read_to_end(&mut request).await.unwrap();
    task.await.unwrap().unwrap();
    let request = String::from_utf8(request).unwrap();
    assert_eq!(request.matches("Cookie: ").count(), 1, "{request}");
    assert!(request.contains("Cookie: a=b; c=d\r\n"), "{request}");
}

#[tokio::test]
async fn writes_content_length_request_body_chunks_as_they_arrive() {
    let (mut reader, mut writer) = tokio::io::duplex(64 * 1024);
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Content-Length".into(), "10".into()),
        ],
        body: Vec::new(),
        body_stream: Some(stream),
        body_chunked: false,
        body_limit: 10,
    };
    let task =
        tokio::spawn(async move { write_proxy_request(&mut writer, &outgoing, "1.1").await });

    sender.send(Bytes::from_static(b"part")).await;
    let mut received = Vec::new();
    read_until(&mut reader, &mut received, b"part").await;
    assert!(
        String::from_utf8_lossy(&received).contains("Content-Length: 10\r\n"),
        "{}",
        String::from_utf8_lossy(&received)
    );

    sender.send(Bytes::from_static(b"remain")).await;
    drop(sender);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn writes_streaming_request_body_with_chunked_upstream_framing() {
    let (mut reader, mut writer) = tokio::io::duplex(64 * 1024);
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Transfer-Encoding".into(), "chunked".into()),
        ],
        body: Vec::new(),
        body_stream: Some(stream),
        body_chunked: true,
        body_limit: 0,
    };
    let task =
        tokio::spawn(async move { write_proxy_request(&mut writer, &outgoing, "1.1").await });

    sender.send(Bytes::from_static(b"part")).await;
    let mut received = Vec::new();
    read_until(&mut reader, &mut received, b"4\r\npart\r\n").await;

    sender.send(Bytes::from_static(b"remain")).await;
    drop(sender);
    task.await.unwrap().unwrap();
    let mut rest = Vec::new();
    reader.read_to_end(&mut rest).await.unwrap();
    received.extend_from_slice(&rest);
    assert!(received.ends_with(b"6\r\nremain\r\n0\r\n\r\n"));
}

#[tokio::test]
async fn classifies_streaming_request_body_limit_errors_as_413() {
    let (_reader, mut writer) = tokio::io::duplex(64 * 1024);
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Transfer-Encoding".into(), "chunked".into()),
        ],
        body: Vec::new(),
        body_stream: Some(stream),
        body_chunked: true,
        body_limit: 5,
    };
    let task =
        tokio::spawn(async move { write_proxy_request(&mut writer, &outgoing, "1.1").await });

    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let error = task.await.unwrap().unwrap_err();
    assert_eq!(crate::http::request_body_error_status(&error), Some(413));
}

#[tokio::test]
async fn drains_unconsumed_streaming_request_body_before_keepalive_request() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18721;
                    location / {
                        proxy_pass http://127.0.0.1:9;
                        proxy_request_buffering off;
                        if ($arg_early = yes) { return 204; }
                    }
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "127.0.0.1:45721".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_io(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), group.address.clone()),
        group,
        state,
    ));

    client
        .write_all(
            b"POST /?early=yes HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\nbody\
              GET /?early=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("keepalive responses should complete")
        .unwrap();
    let response = String::from_utf8_lossy(&response);
    assert_eq!(response.matches("HTTP/1.1 204 ").count(), 2, "{response}");

    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_content_length_request_body_before_client_finishes() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy request streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let head = read_backend_head(&mut reader).await;
        assert!(head.contains("Content-Length: 10\r\n"), "{head}");

        let mut first = [0_u8; 4];
        reader.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"part");
        first_chunk_tx.send(()).unwrap();

        let mut rest = [0_u8; 6];
        reader.read_exact(&mut rest).await.unwrap();
        assert_eq!(&rest, b"remain");
        write_half
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
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
                        listen 127.0.0.1:18717;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_request_buffering off;
                            proxy_read_timeout 5s;
                            add_header X-Body $request_body;
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
    let remote: SocketAddr = "127.0.0.1:45717".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_chunk_rx)
        .await
        .expect("the first request chunk should reach upstream before the client finishes")
        .unwrap();
    client.write_all(b"remain").await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("request streaming response should complete")
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200 "),
        "{}",
        String::from_utf8_lossy(&response)
    );
    assert!(
        response
            .windows(b"X-Body: partremain\r\n".len())
            .any(|window| window == b"X-Body: partremain\r\n"),
        "{}",
        String::from_utf8_lossy(&response)
    );
    assert!(response.ends_with(b"ok"));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_chunked_request_body_with_chunked_upstream_framing() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy request streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = BufReader::new(read_half);
        let head = read_backend_head(&mut reader).await;
        assert!(
            head.to_ascii_lowercase()
                .contains("transfer-encoding: chunked\r\n"),
            "{head}"
        );
        assert_eq!(read_backend_chunk(&mut reader).await, b"part");
        first_chunk_tx.send(()).unwrap();
        assert_eq!(read_backend_chunk(&mut reader).await, b"remain");
        let mut end = String::new();
        reader.read_line(&mut end).await.unwrap();
        assert_eq!(end, "0\r\n");
        let mut trailers = String::new();
        reader.read_line(&mut trailers).await.unwrap();
        assert_eq!(trailers, "\r\n");
        write_half
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
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
                        listen 127.0.0.1:18718;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_request_buffering off;
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
    let remote: SocketAddr = "127.0.0.1:45718".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
              Connection: close\r\n\r\n4\r\npart\r\n",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_chunk_rx)
        .await
        .expect("the first chunked request chunk should reach upstream")
        .unwrap();
    client.write_all(b"6\r\nremain\r\n0\r\n\r\n").await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("chunked request streaming response should complete")
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 200 "),
        "{}",
        String::from_utf8_lossy(&response)
    );

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_chunked_request_body_over_client_limit_while_streaming() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy request streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 1024];
        while stream.read(&mut buffer).await.unwrap_or(0) != 0 {}
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18719;
                        client_max_body_size 5;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_request_buffering off;
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
    let remote: SocketAddr = "127.0.0.1:45719".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
              Connection: close\r\n\r\n4\r\npart\r\n4\r\nmore\r\n0\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("over-limit request should receive a response")
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 413 "),
        "{}",
        String::from_utf8_lossy(&response)
    );

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_backend_head<R>(reader: &mut R) -> String
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

async fn read_backend_chunk<R>(reader: &mut R) -> Vec<u8>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    let size = usize::from_str_radix(line.trim_end(), 16).unwrap();
    let mut data = vec![0_u8; size];
    reader.read_exact(&mut data).await.unwrap();
    let mut crlf = [0_u8; 2];
    reader.read_exact(&mut crlf).await.unwrap();
    assert_eq!(&crlf, b"\r\n");
    data
}

async fn read_until<R>(reader: &mut R, received: &mut Vec<u8>, needle: &[u8])
where
    R: AsyncRead + Unpin,
{
    tokio::time::timeout(Duration::from_secs(1), async {
        while !received
            .windows(needle.len())
            .any(|window| window == needle)
        {
            let mut buffer = [0_u8; 1024];
            let read = reader.read(&mut buffer).await.unwrap();
            assert!(read > 0, "stream ended before expected bytes");
            received.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("expected request bytes before the body stream ended");
}
