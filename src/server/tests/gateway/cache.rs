use super::*;

const LAST_MODIFIED: &str = "Wed, 21 Oct 2015 07:28:00 GMT";

#[tokio::test]
async fn fastcgi_serves_repeated_requests_from_disk_cache() {
    let Some(upstream) = bind_gateway_upstream("fastcgi cache test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_fastcgi_request(&mut stream).await;
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\ncached",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        assert_no_second_connection(&upstream).await;
    });
    let (prefix, config) = test_config(
        &gateway_cache_config("fastcgi", &upstream_address.to_string(), 18151),
        &[],
    );

    assert_gateway_cache_hit(&config).await;

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn uwsgi_serves_repeated_requests_from_disk_cache() {
    let Some(upstream) = bind_gateway_upstream("uwsgi cache test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_uwsgi_request(&mut stream).await;
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\ncached",
            )
            .await
            .unwrap();
        assert_no_second_connection(&upstream).await;
    });
    let (prefix, config) = test_config(
        &gateway_cache_config("uwsgi", &upstream_address.to_string(), 18152),
        &[],
    );

    assert_gateway_cache_hit(&config).await;

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn scgi_serves_repeated_requests_from_disk_cache() {
    let Some(upstream) = bind_gateway_upstream("scgi cache test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_scgi_request(&mut stream).await;
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\r\ncached",
            )
            .await
            .unwrap();
        assert_no_second_connection(&upstream).await;
    });
    let (prefix, config) = test_config(
        &gateway_cache_config("scgi", &upstream_address.to_string(), 18153),
        &[],
    );

    assert_gateway_cache_hit(&config).await;

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_caches_gateway_failure_status_until_validity_expires() {
    let Some(listener) = bind_gateway_upstream("fastcgi negative cache test").await else {
        return;
    };
    let upstream_address = listener.local_addr().unwrap();
    drop(listener);
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    fastcgi_cache_path cache keys_zone=gateway_cache:1m;
                    server {{
                        listen 127.0.0.1:18156;
                        add_header X-Cache-Status $upstream_cache_status always;
                        location / {{
                            fastcgi_pass {upstream_address};
                            fastcgi_cache gateway_cache;
                            fastcgi_cache_key "$request_method$host$request_uri";
                            fastcgi_cache_valid 502 1s;
                            fastcgi_next_upstream off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{first}");
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");

    let second = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        second.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{second}"
    );
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let listener = TcpListener::bind(upstream_address)
        .await
        .unwrap_or_else(|error| panic!("rebind FastCGI upstream: {error}"));
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_fastcgi_request(&mut stream).await;
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
    });
    let recovered = request_body_with_state(&config, state, request).await;
    assert!(recovered.starts_with("HTTP/1.1 200 OK\r\n"), "{recovered}");
    assert!(recovered.ends_with("ok"), "{recovered}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_revalidates_expired_cache_entry_with_conditional_headers() {
    let Some(upstream) = bind_gateway_upstream("fastcgi cache revalidate test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_fastcgi_request(&mut stream).await;
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            format!(
                "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 6\r\n\
                 ETag: \"v1\"\r\nLast-Modified: {LAST_MODIFIED}\r\n\r\ncached"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();

        let (mut stream, _) = upstream.accept().await.unwrap();
        let parameters = read_fastcgi_parameters(&mut stream).await;
        let parameters = parse_fastcgi_test_params(&parameters);
        assert_eq!(
            parameters.get("HTTP_IF_NONE_MATCH").map(String::as_str),
            Some("\"v1\"")
        );
        assert_eq!(
            parameters.get("HTTP_IF_MODIFIED_SINCE").map(String::as_str),
            Some(LAST_MODIFIED)
        );
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 304 Not Modified\r\nETag: \"v1\"\r\n\r\n",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        assert_no_second_connection(&upstream).await;
    });
    let (prefix, config) = test_config(
        &gateway_cache_config_with(
            "fastcgi",
            &upstream_address.to_string(),
            18154,
            "1s",
            "fastcgi_cache_revalidate on;",
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let revalidated = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        revalidated.contains("X-Cache-Status: REVALIDATED\r\n"),
        "{revalidated}"
    );
    assert!(
        revalidated.starts_with("HTTP/1.1 200 OK\r\n"),
        "{revalidated}"
    );
    assert!(revalidated.ends_with("cached"), "{revalidated}");

    let hit = request_body_with_state(&config, state, request).await;
    assert!(hit.contains("X-Cache-Status: HIT\r\n"), "{hit}");
    assert!(hit.ends_with("cached"), "{hit}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn fastcgi_background_update_returns_stale_and_refreshes_cache() {
    let Some(upstream) = bind_gateway_upstream("fastcgi cache background test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (update_started, update_started_rx) = tokio::sync::oneshot::channel();
    let (release_update, release_update_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_fastcgi_request(&mut stream).await;
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\n\r\nold",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();

        let (mut stream, _) = upstream.accept().await.unwrap();
        read_fastcgi_request(&mut stream).await;
        update_started.send(()).unwrap();
        release_update_rx.await.unwrap();
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\n\
              Cache-Control: max-age=60\r\n\r\nnew",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        assert_no_second_connection(&upstream).await;
    });
    let (prefix, config) = test_config(
        &gateway_cache_config_with(
            "fastcgi",
            &upstream_address.to_string(),
            18155,
            "1s",
            "fastcgi_cache_use_stale updating;\n\
             fastcgi_cache_background_update on;\n\
             fastcgi_cache_lock on;\n\
             fastcgi_cache_lock_timeout 2s;",
        ),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("old"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let stale_config = config.clone();
    let stale_state = state.clone();
    let stale =
        tokio::spawn(
            async move { request_body_with_state(&stale_config, stale_state, request).await },
        );
    update_started_rx.await.unwrap();
    let stale = tokio::time::timeout(Duration::from_secs(1), stale)
        .await
        .expect("stale response should not wait for the background update")
        .unwrap();
    assert!(stale.contains("X-Cache-Status: STALE\r\n"), "{stale}");
    assert!(stale.ends_with("old"), "{stale}");

    let updating = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        updating.contains("X-Cache-Status: UPDATING\r\n"),
        "{updating}"
    );
    assert!(updating.ends_with("old"), "{updating}");

    release_update.send(()).unwrap();
    backend.await.unwrap();

    let mut last = String::new();
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last = request_body_with_state(&config, state.clone(), request).await;
        if last.contains("X-Cache-Status: HIT\r\n") {
            break;
        }
    }
    assert!(last.contains("X-Cache-Status: HIT\r\n"), "{last}");
    assert!(last.ends_with("new"), "{last}");

    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_gateway_upstream(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: binding TCP sockets is not permitted");
            None
        }
        Err(error) => panic!("bind upstream: {error}"),
    }
}

fn gateway_cache_config(kind: &str, upstream: &str, listen_port: u16) -> String {
    gateway_cache_config_with(kind, upstream, listen_port, "1m", "")
}

fn gateway_cache_config_with(
    kind: &str,
    upstream: &str,
    listen_port: u16,
    valid: &str,
    extra: &str,
) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                {kind}_cache_path cache keys_zone=gateway_cache:1m;
                server {{
                    listen 127.0.0.1:{listen_port};
                    add_header X-Cache-Status $upstream_cache_status always;
                    location / {{
                        {kind}_pass {upstream};
                        {kind}_cache gateway_cache;
                        {kind}_cache_key "$request_method$host$request_uri";
                        {kind}_cache_valid 200 {valid};
                        {extra}
                    }}
                }}
            }}
        "#
    )
}

async fn assert_gateway_cache_hit(config: &Arc<RuntimeConfig>) {
    let state = test_state(config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("cached"), "{first}");

    let second = request_body_with_state(config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
    assert!(second.ends_with("cached"), "{second}");
}

async fn read_fastcgi_request<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let _ = read_fastcgi_parameters(stream).await;
}

async fn read_fastcgi_parameters<S>(stream: &mut S) -> Vec<u8>
where
    S: AsyncRead + Unpin,
{
    let mut parameters = Vec::new();
    loop {
        let (record_type, content) = read_fastcgi_test_record(stream).await;
        if record_type == FCGI_PARAMS && !content.is_empty() {
            parameters.extend_from_slice(&content);
        }
        if record_type == FCGI_STDIN && content.is_empty() {
            return parameters;
        }
    }
}

async fn read_uwsgi_request<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await.unwrap();
    let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; parameter_length];
    stream.read_exact(&mut parameters).await.unwrap();
}

async fn read_scgi_request<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let length = read_scgi_length(stream).await;
    let mut packet = vec![0_u8; length];
    stream.read_exact(&mut packet).await.unwrap();
    let mut comma = [0_u8; 1];
    stream.read_exact(&mut comma).await.unwrap();
    assert_eq!(comma, [b',']);
}

async fn read_scgi_length<S>(stream: &mut S) -> usize
where
    S: AsyncRead + Unpin,
{
    let mut digits = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.unwrap();
        if byte[0] == b':' {
            break;
        }
        digits.push(byte[0]);
    }
    std::str::from_utf8(&digits).unwrap().parse().unwrap()
}

async fn assert_no_second_connection(listener: &TcpListener) {
    if tokio::time::timeout(Duration::from_millis(300), listener.accept())
        .await
        .is_ok()
    {
        panic!("cache hit unexpectedly reached the upstream");
    }
}
