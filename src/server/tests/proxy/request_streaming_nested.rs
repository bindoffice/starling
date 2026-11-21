use super::*;
use tokio::net::TcpListener;

#[test]
fn nested_streaming_location_keeps_server_wide_body_streaming_enabled() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18730;
                    location /outer/ {
                        location /outer/inner/ {
                            proxy_pass http://127.0.0.1:9;
                            proxy_request_buffering off;
                        }
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(server_may_stream_request_body(server));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_nested_location_body_before_the_client_finishes() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping nested request streaming test: TCP binding is not permitted");
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
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" {
                break;
            }
        }
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
                        listen 127.0.0.1:18730;
                        location /outer/ {{
                            location /outer/inner/ {{
                                proxy_pass http://{upstream};
                                proxy_request_buffering off;
                                proxy_read_timeout 5s;
                            }}
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
    let remote: SocketAddr = "127.0.0.1:45730".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    client
        .write_all(
            b"POST /outer/inner/ HTTP/1.1\r\nHost: localhost\r\nContent-Length: 10\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), first_chunk_rx)
        .await
        .expect("nested location should stream the first request chunk")
        .unwrap();
    client.write_all(b"remain").await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("nested request streaming response should complete")
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200 "));
    assert!(response.ends_with(b"ok"));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
