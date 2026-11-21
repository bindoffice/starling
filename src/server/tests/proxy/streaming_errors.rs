use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn read_timeout_after_commit_truncates_the_committed_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy timeout-after-commit test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 100\r\nConnection: close\r\n\r\nfirst",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18722;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_read_timeout 100ms;
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
    let remote: SocketAddr = "127.0.0.1:45722".parse().unwrap();
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

    let response = read_committed_prefix(&mut client).await;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest))
        .await
        .expect("timed-out upstream should close the committed response")
        .unwrap();
    let response = String::from_utf8([response, rest].concat()).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("\r\n\r\nfirst"), "{response}");
    assert!(!response.contains("502 Bad Gateway"), "{response}");
    assert!(!response.contains("504 Gateway Time-out"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the upstream timeout")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn upstream_truncation_after_commit_truncates_without_rewriting() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy truncation-after-commit test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Content-Length: 100\r\nConnection: close\r\n\r\nfirst",
            )
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
                        listen 127.0.0.1:18723;
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
    let remote: SocketAddr = "127.0.0.1:45723".parse().unwrap();
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

    let response = read_committed_prefix(&mut client).await;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest))
        .await
        .expect("truncated upstream should close the committed response")
        .unwrap();
    let response = String::from_utf8([response, rest].concat()).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("\r\n\r\nfirst"), "{response}");
    assert!(!response.contains("502 Bad Gateway"), "{response}");
    assert!(!response.contains("504 Gateway Time-out"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after upstream truncation")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn unterminated_upstream_trailers_after_commit_truncate_without_rewriting() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping upstream trailer framing test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  4\r\nbody\r\n0\r\nX-Trace: one\r\n",
            )
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
                        listen 127.0.0.1:18734;
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
    let remote: SocketAddr = "127.0.0.1:45734".parse().unwrap();
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
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("incomplete upstream trailers should close the committed response")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("\r\n\r\n4\r\nbody\r\n"), "{response}");
    assert!(!response.contains("0\r\n\r\n"), "{response}");
    assert!(!response.contains("502 Bad Gateway"), "{response}");
    assert!(!response.contains("504 Gateway Time-out"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the trailer framing error")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn unterminated_upstream_trailers_before_commit_return_502() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered trailer framing test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                  Transfer-Encoding: chunked\r\nConnection: close\r\n\r\n\
                  4\r\nbody\r\n0\r\nX-Trace: one\r\n",
            )
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
                        listen 127.0.0.1:18735;
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
    let remote: SocketAddr = "127.0.0.1:45735".parse().unwrap();
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
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered trailer framing failure should receive a response")
        .unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 502 "), "{response}");
    assert!(!response.contains("\r\n\r\n4\r\nbody\r\n"), "{response}");

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the buffered framing error")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_committed_prefix(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response.windows(5).any(|window| window == b"first") {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before committing the response");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first upstream chunk should reach the client before failure");
    response
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
