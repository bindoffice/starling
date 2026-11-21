use super::support::*;
use super::*;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn half_closed_incomplete_body_is_400_before_upstream_connect() {
    let Some((upstream_listener, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let upstream = upstream_listener.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), upstream_listener.accept())
            .await
            .is_ok()
        {
            panic!("an incomplete buffered body must fail before connecting upstream");
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 2;
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:18753;
                        location / {{
                            proxy_pass http://{upstream};
                            client_body_timeout 5s;
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
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("incomplete half-closed body should receive a response")
        .unwrap();
    assert!(
        response.starts_with(b"HTTP/1.1 400 "),
        "{}",
        String::from_utf8_lossy(&response)
    );

    server_task.await.unwrap();
    upstream_task.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "incomplete half-closed body leaked its spool file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn half_close_during_streaming_body_write_cancels_without_retry() {
    let Some((first, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (prefix_seen, prefix_seen_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (stream, _) = first.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        read_backend_head(&mut reader).await;
        let mut prefix = [0_u8; 4];
        reader.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"part");
        prefix_seen.send(()).unwrap();

        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), reader.read(&mut byte))
            .await
            .expect("half-closed client should stop the upstream request body write");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "unexpected upstream body bytes after client half-close: {closed:?}"
        );
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("client half-close still started a second upstream attempt");
        }
    });

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream body_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18754;
                        location / {{
                            proxy_pass http://body_retry;
                            proxy_request_buffering off;
                            proxy_next_upstream error timeout non_idempotent;
                            proxy_next_upstream_tries 2;
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
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), prefix_seen_rx)
        .await
        .expect("upstream should receive the partial request body")
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("streaming half-close should close the downstream connection")
        .unwrap();
    assert!(
        response.is_empty(),
        "a half-closed streaming client must not receive a synthesized response: {}",
        String::from_utf8_lossy(&response)
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the half-close")
        .unwrap();
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_backend_head<R>(reader: &mut R)
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await.unwrap();
        assert!(read != 0, "upstream closed before request headers");
        if line == "\r\n" {
            return;
        }
    }
}

async fn wait_for_temp_cleanup(path: &std::path::Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
