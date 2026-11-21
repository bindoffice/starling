use super::fixtures::*;
use super::*;

#[tokio::test]
async fn slow_upstream_progress_with_a_slow_client_completes_and_cleans_spill() {
    let Some((upstream, backend)) = trickle_spill_upstream(Duration::from_millis(20)).await else {
        return;
    };
    let (prefix, config) = test_config(
        &spill_config_with_timeout("proxy-temp", upstream, 1000, "150ms"),
        &[],
    );
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    let response = match read_response_with_body(&mut client, SLOW_BODY_SIZE).await {
        Ok(response) => response,
        Err(partial) => {
            let error = client.take_error();
            let joined = tokio::time::timeout(Duration::from_secs(1), server).await;
            panic!(
                "timed out after {} response bytes; socket_error={error:?}; \
                 server_result={joined:?}; prefix={}",
                partial.len(),
                String::from_utf8_lossy(&partial[..partial.len().min(256)])
            );
        }
    };
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), SLOW_BODY_SIZE);
    assert!(body.iter().all(|byte| *byte == b'x'));

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("completed slow response should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("slow upstream should finish after its complete response is consumed")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reset_during_a_slow_upstream_spill_cancels_backend_and_cleans_temp() {
    let Some((upstream, backend)) = cancellable_trickle_spill_upstream().await else {
        return;
    };
    let (prefix, config) = test_config(&spill_config("proxy-temp", upstream, 1000), &[]);
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    reset_client(client);

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("client reset should stop a handler waiting on a slow upstream")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), backend)
        .await
        .expect("client reset should cancel a slow upstream between body chunks")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}
