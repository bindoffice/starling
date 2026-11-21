use super::fixtures::*;
use super::*;

#[tokio::test]
async fn deleting_a_live_spill_truncates_the_committed_response_and_cancels_upstream() {
    let Some((upstream, backend)) =
        cancellable_trickle_spill_upstream_with_delay(Duration::from_millis(20)).await
    else {
        return;
    };
    let (prefix, config) = test_config(&spill_config("proxy-temp", upstream, 1000), &[]);
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(b"GET /delete HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    let spilled = temp_files(&temp_root);
    assert!(!spilled.is_empty());
    for path in spilled {
        std::fs::remove_file(path).unwrap();
    }

    let response = read_to_end_within(&mut client, Duration::from_secs(3)).await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(!headers.contains("502 Bad Gateway"), "{headers}");
    assert!(!headers.contains("504 Gateway Time-out"), "{headers}");
    assert!(
        body.len() < SLOW_BODY_SIZE,
        "deleting a live spill must truncate the response, got {} bytes",
        body.len()
    );
    assert!(
        !body.is_empty(),
        "the committed response should retain the prefix already sent downstream"
    );

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("spill read failure should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), backend)
        .await
        .expect("spill read failure should cancel the slow upstream")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}
