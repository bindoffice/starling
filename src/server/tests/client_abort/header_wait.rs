use super::support::*;
use super::*;

#[tokio::test]
async fn client_reset_cancels_upstream_while_waiting_for_response_headers() {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        request_seen_tx.send(()).unwrap();

        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("client reset should cancel the upstream while response headers are pending");
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
                        listen 127.0.0.1:18742;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_read_timeout 30s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("upstream should receive the request before the client reset")
        .unwrap();

    reset_client(client);

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the client reset")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn client_half_close_cancels_upstream_while_waiting_for_response_headers() {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        request_seen_tx.send(()).unwrap();

        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect(
                "client half-close should cancel the upstream while response headers are pending",
            );
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after client half-close: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18751;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_read_timeout 30s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("upstream should receive the request before the client half-close")
        .unwrap();

    half_close_client(client).await;

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the client half-close")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn ignore_client_abort_keeps_waiting_for_response_headers_after_reset() {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        release_rx.await.unwrap();

        let mut buffer = [0_u8; 1];
        let premature =
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer)).await;
        assert!(
            premature.is_err(),
            "ignore_client_abort on cancelled the upstream while headers were pending: \
             {premature:?}"
        );
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nfirstsecond",
            )
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("preserved upstream should close after the complete response");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after preserved response: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18743;
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
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("upstream should receive the request before the client reset")
        .unwrap();

    reset_client(client);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after draining the preserved response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn proxy_store_keeps_waiting_for_response_headers_after_reset() {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (request_seen_tx, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        request_seen_tx.send(()).unwrap();
        release_rx.await.unwrap();

        let mut buffer = [0_u8; 1];
        let premature =
            tokio::time::timeout(Duration::from_millis(200), stream.read(&mut buffer)).await;
        assert!(
            premature.is_err(),
            "proxy_store cancelled the upstream while headers were pending: {premature:?}"
        );
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  5\r\nfirst\r\n6\r\nsecond\r\n0\r\n\r\n",
            )
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("stored upstream should close after the complete response");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read after stored response: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18744;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_store on;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(b"GET /header-store HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), request_seen_rx)
        .await
        .expect("upstream should receive the request before the client reset")
        .unwrap();

    reset_client(client);
    release_tx.send(()).unwrap();

    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .expect("connection handler should finish after preserving the stored response")
        .unwrap();
    backend.await.unwrap();
    let stored = prefix.join("html/header-store");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if stored.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the preserved proxy_store tee should publish its file");
    assert_eq!(std::fs::read(stored).unwrap(), b"firstsecond");
    let _ = std::fs::remove_dir_all(prefix);
}
