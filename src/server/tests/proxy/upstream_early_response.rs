use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn reads_early_upstream_response_after_streaming_body_write_fails() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy early response test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            head.push(byte[0]);
        }

        stream
            .write_all(
                b"HTTP/1.1 413 Payload Too Large\r\n\
                  Content-Length: 9\r\n\
                  Connection: close\r\n\
                  \r\n\
                  too large",
            )
            .await
            .unwrap();
        // Closing with an unread request body makes the next proxy write fail.
        // The already queued response must still be consumed.
        drop(stream);
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
    let remote: SocketAddr = "127.0.0.1:45720".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(128 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let body = vec![b'x'; 64 * 1024];
    let mut request = b"POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
          Connection: close\r\n\r\n"
        .to_vec();
    request.extend_from_slice(format!("{:X}\r\n", body.len()).as_bytes());
    request.extend_from_slice(&body);
    request.extend_from_slice(b"\r\n0\r\n\r\n");
    client.write_all(&request).await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), client.read_to_end(&mut response))
        .await
        .expect("early upstream response should complete")
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
