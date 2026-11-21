use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn streams_unbuffered_chunked_proxy_response_before_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy streaming test: TCP binding is not permitted");
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
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\n\r\n")
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
                        listen 127.0.0.1:18710;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
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
    let remote: SocketAddr = "127.0.0.1:45710".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
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
            assert!(read > 0, "starling closed before streaming the first chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client before upstream EOF");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming response should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.contains("Transfer-Encoding: chunked\r\n"),
        "{response}"
    );
    assert!(
        response.find("first\n").unwrap() < response.find("second\n").unwrap(),
        "{response}"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should close after the response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_content_length_body_before_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy streaming test: TCP binding is not permitted");
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
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                  Content-Length: 11\r\nConnection: close\r\n\r\nfirst",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream.write_all(b"second").await.unwrap();
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18711;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
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
    let remote: SocketAddr = "127.0.0.1:45711".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming the first chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("Content-Length body should stream before upstream EOF");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming response should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.ends_with("firstsecond"), "{response}");
    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn downstream_write_failure_cancels_a_stalled_unbuffered_upstream_body() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy streaming cancellation test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (send_second, wait_for_second) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                  Content-Length: 100\r\nConnection: close\r\n\r\nfirst",
            )
            .await
            .unwrap();
        wait_for_second.await.unwrap();
        stream.write_all(b"second").await.unwrap();

        let mut buffer = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .expect("downstream disconnect should close the stalled upstream connection");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream read: {closed:?}"
        );
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18720;
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
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45720".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before streaming the first chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first chunk should reach the client before downstream closes");

    drop(client);
    send_second.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("upstream producer should stop after the downstream write failure")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP server should stop after downstream disconnect")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_x_accel_buffering_no_response_before_upstream_eof() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy streaming test: TCP binding is not permitted");
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
                  X-Accel-Buffering: no\r\nTransfer-Encoding: chunked\r\n\
                  Connection: close\r\n\r\n6\r\nfirst\n\r\n",
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\n\r\n")
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
                        listen 127.0.0.1:18712;
                        location / {{
                            proxy_pass http://{upstream};
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
    let remote: SocketAddr = "127.0.0.1:45712".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
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
            assert!(read > 0, "starling closed before streaming the first chunk");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client before upstream EOF");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming response should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(
        !response.contains("X-Accel-Buffering:"),
        "X-Accel-Buffering must not be forwarded"
    );
    assert!(
        response.find("first\n").unwrap() < response.find("second\n").unwrap(),
        "{response}"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should close after the response")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_upstream_chunked_trailers_when_enabled() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy streaming test: TCP binding is not permitted");
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
        stream
            .write_all(b"7\r\nsecond\n\r\n0\r\nX-Stream-Trailer: yes\r\n\r\n")
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
                        listen 127.0.0.1:18716;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_pass_trailers on;
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
    let remote: SocketAddr = "127.0.0.1:45716".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
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
            assert!(read > 0, "starling closed before streaming trailer data");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client before trailers");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming response with trailers should complete")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.contains("X-Stream-Trailer: yes\r\n"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP server should close after the response")
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
