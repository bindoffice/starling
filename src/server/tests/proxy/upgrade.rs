use super::*;

async fn upgrade_listener() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy upgrade test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind upstream: {error}"),
    }
}

fn upgrade_config(upstream: SocketAddr) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                log_format upgrade "$bytes_sent $body_bytes_sent $sent_http_connection";
                access_log logs/upgrade.log upgrade;
                server {{
                    listen 127.0.0.1:18231;
                    location / {{
                        proxy_pass http://{upstream};
                        proxy_http_version 1.1;
                        proxy_set_header Upgrade $http_upgrade;
                        proxy_set_header Connection "Upgrade";
                        proxy_read_timeout 2s;
                        send_timeout 2s;
                    }}
                }}
            }}
        "#
    )
}

#[tokio::test]
async fn relays_pipelined_and_prefetched_upgrade_bytes_and_logs_them() {
    let Some(listener) = upgrade_listener().await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_until_headers(&mut stream).await;
        assert!(request.windows(12).any(|window| window == b"Upgrade: foo"));
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\n\
                  Upgrade: websocket\r\n\
                  Connection: Upgrade\r\n\r\n\
                  server-prefetch\r\n",
            )
            .await
            .unwrap();
        let mut pipelined = [0_u8; 12];
        stream.read_exact(&mut pipelined).await.unwrap();
        assert_eq!(&pipelined, b"client-one\r\n");
        stream.write_all(b"server-after\r\n").await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let (prefix, config) = test_config(&upgrade_config(upstream), &[]);
    let request = "GET / HTTP/1.1\r\n\
                   Host: localhost\r\n\
                   Upgrade: foo\r\n\
                   Connection: Upgrade\r\n\r\n\
                   client-one\r\n";
    let response = request_bytes(&config, request).await;
    backend.await.unwrap();

    let (head, body) = split_http_response(&response);
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{head}"
    );
    assert!(head.contains("Upgrade: websocket\r\n"), "{head}");
    assert!(head.contains("Connection: upgrade\r\n"), "{head}");
    assert_eq!(body, b"server-prefetch\r\nserver-after\r\n");

    let log = std::fs::read_to_string(prefix.join("logs/upgrade.log")).unwrap();
    assert_eq!(log, format!("{} {} upgrade\n", response.len(), body.len()));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn upstream_101_without_a_client_upgrade_header_is_not_relayed() {
    let Some(listener) = upgrade_listener().await else {
        return;
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let _ = read_until_headers(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\n\
                  Upgrade: websocket\r\n\
                  Connection: Upgrade\r\n\r\n\
                  must-not-be-forwarded\r\n",
            )
            .await
            .unwrap();
        let mut byte = [0_u8; 1];
        let _ = stream.read(&mut byte).await;
    });

    let (prefix, config) = test_config(&upgrade_config(upstream), &[]);
    let response = request_bytes(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\n\r\n",
    )
    .await;
    backend.await.unwrap();

    let (head, body) = split_http_response(&response);
    assert!(
        head.starts_with("HTTP/1.1 101 Switching Protocols\r\n"),
        "{head}"
    );
    assert!(body.is_empty(), "{body:?}");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_until_headers(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0);
        request.extend_from_slice(&buffer[..read]);
    }
    request
}
