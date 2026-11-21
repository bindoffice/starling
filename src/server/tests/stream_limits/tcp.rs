use super::*;

#[tokio::test]
async fn enforces_stream_connection_limits_for_the_full_tcp_session() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream limit_conn test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind stream limit_conn backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        {
            let (mut first, _) = backend.accept().await.unwrap();
            let mut request = [0_u8; 4];
            first.read_exact(&mut request).await.unwrap();
            first.write_all(b"pong").await.unwrap();
            let mut remaining = Vec::new();
            first.read_to_end(&mut remaining).await.unwrap();
        }

        let (mut third, _) = backend.accept().await.unwrap();
        third.read_exact(&mut [0_u8; 4]).await.unwrap();
        third.write_all(b"pong").await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                limit_conn_zone $binary_remote_addr zone=perip:10m;
                limit_conn perip 1;
                log_format limits '$status $limit_conn_status';
                access_log logs/limits.log limits;
                server {{
                    listen 127.0.0.1:13334;
                    proxy_pass {backend_address};
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.9:45679".parse().unwrap();

    let (mut first_client, first_inbound) = tokio::io::duplex(1024);
    let first_route = route.clone();
    let first_state = state.clone();
    let first = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(first_inbound, &addresses, first_route, first_state).await;
    });
    first_client.write_all(b"ping").await.unwrap();
    let mut response = [0_u8; 4];
    tokio::time::timeout(
        Duration::from_secs(2),
        first_client.read_exact(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&response, b"pong");

    let (mut second_client, second_inbound) = tokio::io::duplex(1024);
    let second_route = route.clone();
    let second_state = state.clone();
    let second = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(second_inbound, &addresses, second_route, second_state).await;
    });
    let mut rejected = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(2),
        second_client.read_to_end(&mut rejected),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(rejected.is_empty());
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap();

    drop(first_client);
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();

    let (mut third_client, third_inbound) = tokio::io::duplex(1024);
    let third_state = state.clone();
    let third = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(third_inbound, &addresses, route, third_state).await;
    });
    third_client.write_all(b"ping").await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        third_client.read_exact(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&response, b"pong");
    drop(third_client);
    tokio::time::timeout(Duration::from_secs(2), third)
        .await
        .unwrap()
        .unwrap();
    backend_task.await.unwrap();

    let log = std::fs::read_to_string(prefix.join("logs/limits.log")).unwrap();
    assert!(log.lines().any(|line| line == "200 PASSED"));
    assert!(log.lines().any(|line| line == "503 REJECTED"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stream_limit_conn_dry_run_allows_the_session() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream limit_conn dry-run test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind stream limit_conn dry-run backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut first, _) = backend.accept().await.unwrap();
        let first = tokio::spawn(async move {
            let mut request = [0_u8; 4];
            first.read_exact(&mut request).await.unwrap();
            first.write_all(b"pong").await.unwrap();
            let mut remaining = Vec::new();
            first.read_to_end(&mut remaining).await.unwrap();
        });
        let (mut second, _) = backend.accept().await.unwrap();
        second.read_exact(&mut [0_u8; 4]).await.unwrap();
        second.write_all(b"pong").await.unwrap();
        drop(second);
        first.await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                limit_conn_zone $binary_remote_addr zone=perip:10m;
                limit_conn perip 1;
                limit_conn_dry_run on;
                log_format limits '$status $limit_conn_status';
                access_log logs/limits-dry.log limits;
                server {{
                    listen 127.0.0.1:13335;
                    proxy_pass {backend_address};
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.10:45679".parse().unwrap();

    let (mut first_client, first_inbound) = tokio::io::duplex(1024);
    let first_route = route.clone();
    let first_state = state.clone();
    let first = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(first_inbound, &addresses, first_route, first_state).await;
    });
    first_client.write_all(b"ping").await.unwrap();
    let mut response = [0_u8; 4];
    tokio::time::timeout(
        Duration::from_secs(2),
        first_client.read_exact(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&response, b"pong");

    let (mut second_client, second_inbound) = tokio::io::duplex(1024);
    let second = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(second_inbound, &addresses, route, state).await;
    });
    second_client.write_all(b"ping").await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        second_client.read_exact(&mut response),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&response, b"pong");

    drop(second_client);
    tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .unwrap()
        .unwrap();
    drop(first_client);
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .unwrap()
        .unwrap();
    backend_task.await.unwrap();

    let log = std::fs::read_to_string(prefix.join("logs/limits-dry.log")).unwrap();
    assert!(log.lines().any(|line| line == "200 PASSED"));
    assert!(log.lines().any(|line| line == "200 REJECTED_DRY_RUN"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stream_limit_conn_skips_empty_and_oversized_keys() {
    let long_key = "x".repeat(256);
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                limit_conn_zone $unset zone=empty:10m;
                limit_conn_zone $long_key zone=long:10m;
                limit_conn empty 1;
                limit_conn long 1;
                server {{
                    listen 127.0.0.1:13336;
                    set $long_key "{long_key}";
                    return "[$limit_conn_status]";
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.11:45679".parse().unwrap();

    for _ in 0..2 {
        let (mut client, inbound) = tokio::io::duplex(1024);
        let route = route.clone();
        let state = state.clone();
        let task = tokio::spawn(async move {
            let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
            handle_stream_connection(inbound, &addresses, route, state).await;
        });
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert_eq!(&response, b"[]");
        task.await.unwrap();
    }
    let _ = std::fs::remove_dir_all(prefix);
}
