use super::fixtures::*;
use super::*;
use crate::server::run::reload;

#[tokio::test]
async fn slow_client_reset_cleans_a_buffered_response_spill_and_cancels_upstream() {
    let Some((upstream, backend)) = slow_spill_upstream().await else {
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
        .expect("client reset should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("client reset should cancel the stalled upstream response")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}
#[tokio::test]
async fn reload_during_a_slow_spill_keeps_the_original_temp_root_until_cancel_cleanup() {
    let Some((upstream, backend)) = slow_spill_upstream().await else {
        return;
    };
    let (prefix, config) = test_config(&spill_config("temp-before-reload", upstream, 1000), &[]);
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state.clone());

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let old_temp_root = prefix.join("temp-before-reload");
    wait_for_generated_temp_file(&old_temp_root, true).await;
    std::fs::write(
        &config_path,
        spill_config("temp-after-reload", upstream, 1001),
    )
    .unwrap();
    reload(&state, &config_path, &prefix).await;
    let snapshot = state.config_snapshot().await;
    assert_eq!(
        snapshot.config.http.as_ref().unwrap().servers[0]
            .context
            .client_max_body_size,
        1001
    );
    assert!(
        !temp_files(&old_temp_root).is_empty(),
        "reload must not consume or remove the in-flight request's spill"
    );

    reset_client(client);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("client reset after reload should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("client reset after reload should cancel the upstream")
        .unwrap();
    wait_for_generated_temp_file(&old_temp_root, false).await;
    assert!(
        temp_files(&prefix.join("temp-after-reload")).is_empty(),
        "the reloaded config must not receive stale files from the old request"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
