use super::*;

#[tokio::test]
async fn client_reset_during_h2_spool_removes_the_partial_file() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            client_body_buffer_size 2;
            client_body_temp_path body_temp;
            server {
                listen 127.0.0.1:18661 http2;
                location / { return 204; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45685".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(connection);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .header("content-length", 1024 * 1024)
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from(vec![b'x'; 16 * 1024]), false)
        .unwrap();

    let temp_root = prefix.join("body_temp");
    let mut observed = false;
    for _ in 0..200 {
        if contains_generated_temp_file(&temp_root) {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        observed,
        "HTTP/2 request body should spill before the client reset"
    );

    body.send_reset(h2::Reason::CANCEL);
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("reset response should complete");
    assert!(response.is_err());

    let mut removed = false;
    for _ in 0..200 {
        if !contains_generated_temp_file(&temp_root) {
            removed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        removed,
        "HTTP/2 cancellation must remove the partial spool file"
    );

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/2 connection should close after the client disconnects")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
