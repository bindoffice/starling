use super::*;

#[tokio::test]
async fn fastcgi_buffered_response_spills_for_a_slow_client() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping gateway buffering window test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind FastCGI upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "g".repeat(32 * 1024);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            format!(
                "Status: 200 OK\r\nContent-Type: text/plain\r\n\
                 Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
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
                        listen 127.0.0.1:18811;
                        location / {{
                            fastcgi_pass {upstream};
                            fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                            fastcgi_buffers 2 4k;
                            fastcgi_busy_buffers_size 4k;
                            fastcgi_temp_file_write_size 4k;
                            fastcgi_temp_path gateway-temp;
                            fastcgi_max_temp_file_size 16k;
                            fastcgi_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45811".parse().unwrap();
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

    let temp_dir = prefix.join("gateway-temp");
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
    .expect("slow client should force a FastCGI temp-file spill");

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered FastCGI response should complete after the client reads")
        .unwrap();
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(response_body, expected.as_bytes());

    server_task.await.unwrap();
    backend.await.unwrap();
    assert!(
        !temp_dir.exists() || std::fs::read_dir(&temp_dir).unwrap().next().is_none(),
        "gateway buffering temp file was not cleaned up"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
