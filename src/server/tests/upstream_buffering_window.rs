use super::support::*;
use super::*;

#[tokio::test]
async fn intersects_slow_clients_with_memory_buffers_and_temp_files() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffering window test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "x".repeat(16 * 1024);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "upstream request closed before headers");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
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
                        listen 127.0.0.1:18810;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_file_write_size 4k;
                            proxy_temp_path proxy-temp;
                            proxy_max_temp_file_size 16k;
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
    let remote: SocketAddr = "127.0.0.1:45810".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(1024);
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
    client.shutdown().await.unwrap();

    let temp_dir = prefix.join("proxy-temp");
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_dir(&temp_dir)
                .ok()
                .is_some_and(|mut entries| entries.next().is_some())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("slow client should force a temp-file spill");

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered response should complete after the client reads")
        .unwrap();
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(response_body, expected.as_bytes());

    server_task.await.unwrap();
    backend.await.unwrap();
    assert!(
        !temp_dir.exists() || std::fs::read_dir(&temp_dir).unwrap().next().is_none(),
        "buffering temp file was not cleaned up"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
