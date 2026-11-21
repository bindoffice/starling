use super::*;

#[tokio::test]
async fn stream_limit_conn_holds_udp_sessions_and_skips_invalid_keys() {
    let upstream = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(upstream) => upstream,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP stream limit_conn test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind UDP stream limit_conn upstream: {err}"),
    };
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP stream limit_conn test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind UDP stream limit_conn listener: {err}"),
    };
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_address = listener.local_addr().unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let client_address = client.local_addr().unwrap();
    let long_key = "x".repeat(256);
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                limit_conn_zone $binary_remote_addr zone=perip:10m;
                limit_conn_zone $unset zone=empty:10m;
                limit_conn_zone $long_key zone=long:10m;
                limit_conn perip 1;
                limit_conn empty 1;
                limit_conn long 1;
                log_format limits '$status $limit_conn_status';
                access_log logs/udp-limits.log limits;
                server {{
                    listen {listen_address} udp;
                    set $long_key "{long_key}";
                    proxy_pass {upstream_address};
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let listener = Arc::new(listener);
    let (_first_shutdown_sender, first_shutdown) = watch::channel(ShutdownMode::Running);
    let (first_sender, first_receiver) = mpsc::channel(4);
    let upstream_task = tokio::spawn(async move {
        let mut datagram = [0_u8; 64];
        let (size, remote) = upstream.recv_from(&mut datagram).await.unwrap();
        assert_eq!(&datagram[..size], b"first");
        upstream.send_to(b"reply", remote).await.unwrap();
    });
    let first = tokio::spawn(handle_stream_udp_session(
        listener.clone(),
        first_receiver,
        client_address,
        route.clone(),
        state.clone(),
        first_shutdown,
    ));
    first_sender.send(b"first".to_vec()).await.unwrap();
    let mut response = [0_u8; 128];
    let (size, from) =
        tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, listen_address);
    assert_eq!(&response[..size], b"reply");

    let (_second_shutdown_sender, second_shutdown) = watch::channel(ShutdownMode::Running);
    let (second_sender, second_receiver) = mpsc::channel(4);
    let second = tokio::spawn(handle_stream_udp_session(
        listener,
        second_receiver,
        client_address,
        route,
        state,
        second_shutdown,
    ));
    second_sender.send(b"second".to_vec()).await.unwrap();
    drop(second_sender);
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), client.recv_from(&mut response))
            .await
            .is_err()
    );

    drop(first_sender);
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let log = std::fs::read_to_string(prefix.join("logs/udp-limits.log")).unwrap();
    assert!(log.lines().any(|line| line == "200 PASSED"));
    assert!(log.lines().any(|line| line == "503 REJECTED"));
    let _ = std::fs::remove_dir_all(prefix);
}
