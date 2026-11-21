use super::*;

#[tokio::test]
async fn uwsgi_caches_gateway_failure_status_until_validity_expires() {
    run_uwsgi_negative_cache_case(18158).await;
}

#[tokio::test]
async fn scgi_caches_gateway_failure_status_until_validity_expires() {
    run_scgi_negative_cache_case(18159).await;
}

#[tokio::test]
async fn uwsgi_intercepted_error_is_served_from_negative_cache() {
    run_intercepted_error_case("uwsgi", 18160).await;
}

#[tokio::test]
async fn fastcgi_intercepted_error_is_served_from_negative_cache() {
    run_fastcgi_intercepted_error_case(18162).await;
}

#[tokio::test]
async fn scgi_intercepted_error_is_served_from_negative_cache() {
    run_intercepted_error_case("scgi", 18161).await;
}

async fn run_uwsgi_negative_cache_case(listen_port: u16) {
    let Some(listener) = bind_negative_cache_upstream("uWSGI negative cache").await else {
        return;
    };
    let upstream_address = listener.local_addr().unwrap();
    drop(listener);
    let (prefix, config) = test_config(
        &negative_cache_config("uwsgi", &upstream_address.to_string(), listen_port),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    assert_negative_cache_miss_and_hit(&config, state.clone(), request).await;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let listener = TcpListener::bind(upstream_address)
        .await
        .unwrap_or_else(|error| panic!("rebind uWSGI upstream: {error}"));
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_uwsgi_header(&mut stream).await;
        stream
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });

    let recovered = request_body_with_state(&config, state, request).await;
    assert!(recovered.starts_with("HTTP/1.1 200 OK\r\n"), "{recovered}");
    assert!(
        recovered.contains("X-Cache-Status: MISS\r\n"),
        "{recovered}"
    );
    assert!(recovered.ends_with("ok"), "{recovered}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn run_scgi_negative_cache_case(listen_port: u16) {
    let Some(listener) = bind_negative_cache_upstream("SCGI negative cache").await else {
        return;
    };
    let upstream_address = listener.local_addr().unwrap();
    drop(listener);
    let (prefix, config) = test_config(
        &negative_cache_config("scgi", &upstream_address.to_string(), listen_port),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    assert_negative_cache_miss_and_hit(&config, state.clone(), request).await;

    tokio::time::sleep(Duration::from_millis(10)).await;
    let listener = TcpListener::bind(upstream_address)
        .await
        .unwrap_or_else(|error| panic!("rebind SCGI upstream: {error}"));
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_scgi_header(&mut stream).await;
        stream
            .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });

    let recovered = request_body_with_state(&config, state, request).await;
    assert!(recovered.starts_with("HTTP/1.1 200 OK\r\n"), "{recovered}");
    assert!(
        recovered.contains("X-Cache-Status: MISS\r\n"),
        "{recovered}"
    );
    assert!(recovered.ends_with("ok"), "{recovered}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn run_intercepted_error_case(kind: &str, listen_port: u16) {
    let Some(listener) = bind_negative_cache_upstream(&format!("{kind} intercepted error")).await
    else {
        return;
    };
    let upstream_address = listener.local_addr().unwrap();
    let backend_kind = kind.to_owned();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_gateway_header(&backend_kind, &mut stream).await;
        stream
            .write_all(
                b"Status: 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 14\r\n\r\nupstream-error",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
        assert_no_second_connection(&listener).await;
    });
    let (prefix, config) = test_config(
        &intercepted_error_config(kind, &upstream_address.to_string(), listen_port),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    assert!(first.ends_with("fallback-ok"), "{first}");
    assert!(!first.contains("upstream-error"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.ends_with("fallback-ok"), "{second}");
    assert!(!second.contains("upstream-error"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn run_fastcgi_intercepted_error_case(listen_port: u16) {
    let Some(listener) = bind_negative_cache_upstream("FastCGI intercepted error").await else {
        return;
    };
    let upstream_address = listener.local_addr().unwrap();
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
            b"Status: 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 14\r\n\r\nupstream-error",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        assert_no_second_connection(&listener).await;
    });
    let (prefix, config) = test_config(
        &intercepted_error_config("fastcgi", &upstream_address.to_string(), listen_port),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
    assert!(first.ends_with("fallback-ok"), "{first}");
    assert!(!first.contains("upstream-error"), "{first}");

    let second = request_body_with_state(&config, state, request).await;
    assert!(second.starts_with("HTTP/1.1 200 OK\r\n"), "{second}");
    assert!(second.ends_with("fallback-ok"), "{second}");
    assert!(!second.contains("upstream-error"), "{second}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn assert_negative_cache_miss_and_hit(
    config: &Arc<RuntimeConfig>,
    state: SharedState,
    request: &str,
) {
    let first = request_body_with_state(config, state.clone(), request).await;
    assert!(first.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{first}");
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");

    let second = request_body_with_state(config, state, request).await;
    assert!(
        second.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{second}"
    );
    assert!(second.contains("X-Cache-Status: HIT\r\n"), "{second}");
}

async fn bind_negative_cache_upstream(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: binding TCP sockets is not permitted");
            None
        }
        Err(error) => panic!("bind {label} upstream: {error}"),
    }
}

async fn read_uwsgi_header(stream: &mut TcpStream) {
    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).await.unwrap();
    let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; parameter_length];
    stream.read_exact(&mut parameters).await.unwrap();
}

async fn read_scgi_header(stream: &mut TcpStream) {
    let mut length = Vec::new();
    loop {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).await.unwrap();
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = std::str::from_utf8(&length)
        .unwrap()
        .parse::<usize>()
        .unwrap();
    let mut parameters = vec![0_u8; length];
    stream.read_exact(&mut parameters).await.unwrap();
}

async fn read_gateway_header(kind: &str, stream: &mut TcpStream) {
    match kind {
        "uwsgi" => read_uwsgi_header(stream).await,
        "scgi" => read_scgi_header(stream).await,
        _ => panic!("unsupported gateway {kind}"),
    }
}

async fn assert_no_second_connection(listener: &TcpListener) {
    if tokio::time::timeout(Duration::from_millis(300), listener.accept())
        .await
        .is_ok()
    {
        panic!("negative cache unexpectedly reached the upstream");
    }
}

fn negative_cache_config(kind: &str, upstream: &str, listen_port: u16) -> String {
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
                        {kind}_cache_valid 502 1ms;
                        {kind}_next_upstream off;
                    }}
                }}
            }}
        "#
    )
}

fn intercepted_error_config(kind: &str, upstream: &str, listen_port: u16) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                {kind}_cache_path cache keys_zone=gateway_cache:1m;
                server {{
                    listen 127.0.0.1:{listen_port};
                    error_page 404 /fallback;
                    location / {{
                        {kind}_pass {upstream};
                        {kind}_cache gateway_cache;
                        {kind}_cache_key "$request_method$host$request_uri";
                        {kind}_cache_valid 404 1m;
                        {kind}_intercept_errors on;
                        {kind}_next_upstream off;
                    }}
                    location = /fallback {{
                        return 200 "fallback-ok";
                    }}
                }}
            }}
        "#
    )
}
