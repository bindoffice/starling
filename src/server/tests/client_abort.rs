use super::support::*;
use super::*;
use tokio::sync::mpsc;

mod gateway;
mod header_wait;
mod request_body_gateway;
mod request_body_h2;
mod request_body_wait;
mod support;

#[tokio::test]
async fn drains_a_preserved_response_stream_after_client_abort() {
    let (sender, receiver) = mpsc::channel(1);
    let mut response = Response::new(200);
    response.body = ResponseBody::Stream(ResponseStream::new(receiver, None, false));
    response.mark_preserve_on_client_abort();

    let drain = drain_after_client_abort(&mut response).expect("preserved stream should drain");
    assert!(matches!(response.body, ResponseBody::Empty));
    sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"cached"))))
        .await
        .unwrap();
    drop(sender);

    tokio::time::timeout(Duration::from_secs(1), drain)
        .await
        .expect("drain task should finish")
        .expect("drain task should not panic");
}

#[tokio::test]
async fn does_not_drain_a_stream_without_the_abort_policy() {
    let (_sender, receiver) = mpsc::channel(1);
    let mut response = Response::new(200);
    response.body = ResponseBody::Stream(ResponseStream::new(receiver, None, false));

    assert!(drain_after_client_abort(&mut response).is_none());
    assert!(matches!(response.body, ResponseBody::Stream(_)));
}

#[tokio::test]
async fn preserves_and_drains_a_stream_wrapped_in_segments() {
    let (sender, receiver) = mpsc::channel(1);
    let stream = ResponseStream::new(receiver, None, false);
    let mut response = Response::new(200);
    response.body = ResponseBody::Segments {
        source: Box::new(ResponseBody::Stream(stream)),
        segments: Vec::new(),
    };
    response.mark_preserve_on_client_abort();
    assert!(response.preserve_on_client_abort());

    let drain = drain_after_client_abort(&mut response).expect("wrapped stream should drain");
    sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"ranged"))))
        .await
        .unwrap();
    drop(sender);
    tokio::time::timeout(Duration::from_secs(1), drain)
        .await
        .expect("segmented drain task should finish")
        .expect("segmented drain task should not panic");
}

#[tokio::test]
async fn proxy_ignore_client_abort_keeps_the_upstream_until_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping client abort drain test: TCP binding is not permitted");
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
        stream.write_all(b"7\r\nsecond\n\r\n").await.unwrap();

        let mut buffer = [0_u8; 1];
        let premature_close =
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer)).await;
        assert!(
            premature_close.is_err(),
            "ignore_client_abort on stopped draining the upstream: {premature_close:?}"
        );
        stream
            .write_all(b"6\r\nthird\n\r\n0\r\n\r\n")
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("drained upstream should close after the complete response");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after draining: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18740;
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
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45740".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(6).any(|window| window == b"first\n") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before the first streamed chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client");

    drop(client);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the downstream abort")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("backend should observe the drained upstream response")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn client_reset_cancels_a_stalled_upstream_before_the_next_write() {
    let upstream_listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping client reset test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let frontend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  6\r\nfirst\n\r\n",
            )
            .await
            .unwrap();
        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("client reset should cancel the stalled upstream without more data");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after client reset: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18741;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_read_timeout 30s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server_task = tokio::spawn(async move {
        let (stream, remote) = frontend_listener.accept().await.unwrap();
        let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
        let local = stream
            .local_addr()
            .map(ListenAddress::Inet)
            .unwrap_or_else(|_| group.address.clone());
        handle_io_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
            None,
            control,
        )
        .await;
    });

    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(6).any(|window| window == b"first\n") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before the first streamed chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client");

    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should observe the client reset")
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
