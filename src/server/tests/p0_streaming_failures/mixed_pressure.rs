use tokio::io::{AsyncBufReadExt, BufReader};

use crate::server::run::reload;

use super::fixtures::*;
use super::*;

#[tokio::test]
async fn half_closed_slow_client_returns_the_upstream_connection_to_the_pool() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut stream = BufReader::new(stream);
        read_request_head_and_body(&mut stream, b"body").await;
        write_slow_response(stream.get_mut()).await;

        let head = read_buffered_request_head(&mut stream).await;
        assert!(
            head.starts_with("GET /pool-second HTTP/1.1\r\n"),
            "pooled backend did not receive the second request:\n{head}"
        );
        write_slow_response(stream.get_mut()).await;
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp",
            &format!("server {upstream_address}; keepalive 2;"),
            1000,
            "150ms",
            "proxy_ignore_client_abort on;",
        ),
        &[],
    );
    let state = test_state(&config);
    let temp_root = prefix.join("proxy-temp");

    let first_frontend = bind_frontend().await;
    let first_address = first_frontend.local_addr().unwrap();
    let first_server = spawn_frontend(first_frontend, &config, state.clone());
    let mut first_client = TcpStream::connect(first_address).await.unwrap();
    socket2::SockRef::from(&first_client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    first_client
        .write_all(
            b"POST /half-close HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\
              Connection: close\r\n\r\nbody",
        )
        .await
        .unwrap();
    first_client.shutdown().await.unwrap();

    wait_for_generated_temp_file(&temp_root, true).await;
    let first = read_response_with_body(&mut first_client, SLOW_BODY_SIZE)
        .await
        .expect("half-closed slow client should receive the complete response");
    assert_complete_response(&first);
    wait_for_generated_temp_file(&temp_root, false).await;
    tokio::time::timeout(Duration::from_secs(2), first_server)
        .await
        .expect("half-closed client handler should stop after the response")
        .unwrap();
    wait_for_pooled_connection(&state).await;

    let second_frontend = bind_frontend().await;
    let second_address = second_frontend.local_addr().unwrap();
    let second_server = spawn_frontend(second_frontend, &config, state);
    let second = request_slow_response(second_address, "/pool-second").await;
    assert_complete_response(&second);
    wait_for_generated_temp_file(&temp_root, false).await;
    tokio::time::timeout(Duration::from_secs(2), second_server)
        .await
        .expect("second client handler should stop after Connection: close")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("the half-closed request should preserve upstream keepalive reuse")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reset_drops_a_reused_pool_connection_before_reload_opens_a_new_one() {
    let Some(upstream) = bind_backend().await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let head = read_request_head(&mut stream).await;
        assert!(
            head.starts_with("GET /prime HTTP/1.1\r\n"),
            "unexpected priming request:\n{head}"
        );
        write_probe_response(&mut stream, b"prime", "keep-alive").await;

        let head = read_request_head(&mut stream).await;
        assert!(
            head.starts_with("GET /cancel HTTP/1.1\r\n"),
            "pooled backend did not receive the cancellation request:\n{head}"
        );
        assert_slow_response_is_reset(&mut stream).await;
        drop(stream);

        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), upstream.accept())
            .await
            .expect("reload should open a new upstream connection after the reset")
            .unwrap();
        let head = read_request_head(&mut stream).await;
        assert!(
            head.starts_with("GET /after-reload HTTP/1.1\r\n"),
            "new generation did not open a fresh backend connection:\n{head}"
        );
        write_probe_response(&mut stream, b"new-generation", "close").await;
    });

    let (prefix, config) = test_config(
        &spill_config_with_named_upstream(
            "proxy-temp-old",
            &format!("server {upstream_address}; keepalive 2;"),
            1000,
            "150ms",
            "",
        ),
        &[],
    );
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");

    let first_frontend = bind_frontend().await;
    let first_address = first_frontend.local_addr().unwrap();
    let first_server = spawn_frontend(first_frontend, &config, state.clone());
    let first = request_probe(first_address, "/prime").await;
    assert_eq!(first, b"prime");
    tokio::time::timeout(Duration::from_secs(2), first_server)
        .await
        .expect("priming request handler should stop after Connection: close")
        .unwrap();
    wait_for_pooled_connection(&state).await;

    let second_frontend = bind_frontend().await;
    let second_address = second_frontend.local_addr().unwrap();
    let second_server = spawn_frontend(second_frontend, &config, state.clone());
    let mut second_client = TcpStream::connect(second_address).await.unwrap();
    socket2::SockRef::from(&second_client)
        .set_recv_buffer_size(1024)
        .unwrap();
    second_client
        .write_all(b"GET /cancel HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let old_temp_root = prefix.join("proxy-temp-old");
    wait_for_generated_temp_file(&old_temp_root, true).await;
    reset_client(second_client);
    tokio::time::timeout(Duration::from_secs(2), second_server)
        .await
        .expect("reset should stop the pooled response handler")
        .unwrap();
    wait_for_generated_temp_file(&old_temp_root, false).await;
    wait_for_no_pooled_connection(&state).await;

    std::fs::write(
        &config_path,
        spill_config_with_named_upstream(
            "proxy-temp-new",
            &format!("server {upstream_address}; keepalive 2;"),
            1001,
            "150ms",
            "",
        ),
    )
    .unwrap();
    reload(&state, &config_path, &prefix).await;
    assert_eq!(
        state
            .config_snapshot()
            .config
            .http
            .as_ref()
            .unwrap()
            .servers[0]
            .context
            .client_max_body_size,
        1001
    );

    let third_frontend = bind_frontend().await;
    let third_address = third_frontend.local_addr().unwrap();
    let third_server = spawn_frontend(third_frontend, &config, state);
    let third = request_probe(third_address, "/after-reload").await;
    assert_eq!(third, b"new-generation");
    tokio::time::timeout(Duration::from_secs(2), third_server)
        .await
        .expect("post-reload request handler should stop after Connection: close")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("backend should observe reset then a new generation connection")
        .unwrap();
    assert!(
        temp_files(&prefix.join("proxy-temp-new")).is_empty(),
        "new generation temp root should remain clean"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_head_and_body(stream: &mut BufReader<TcpStream>, expected_body: &[u8]) {
    let mut line = String::new();
    let mut content_length = None;
    loop {
        line.clear();
        let read = stream.read_line(&mut line).await.unwrap();
        assert!(read != 0, "upstream closed before request headers");
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>().unwrap());
        }
    }
    assert_eq!(content_length, Some(expected_body.len()));
    let mut body = vec![0_u8; expected_body.len()];
    stream.read_exact(&mut body).await.unwrap();
    assert_eq!(body, expected_body);
}

async fn read_buffered_request_head(stream: &mut BufReader<TcpStream>) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "pooled upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(request).expect("upstream request headers should be UTF-8")
}

async fn write_slow_response(stream: &mut TcpStream) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {SLOW_BODY_SIZE}\r\nConnection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let chunk = vec![b'x'; SLOW_CHUNK_SIZE];
    for _ in 0..SLOW_BODY_SIZE / SLOW_CHUNK_SIZE {
        stream.write_all(&chunk).await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    stream.flush().await.unwrap();
}

async fn write_probe_response(stream: &mut TcpStream, body: &[u8], connection: &str) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {}\r\nConnection: {connection}\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.write_all(body).await.unwrap();
    stream.flush().await.unwrap();
}

async fn assert_slow_response_is_reset(stream: &mut TcpStream) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Length: {SLOW_BODY_SIZE}\r\nConnection: keep-alive\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let chunk = vec![b'x'; SLOW_CHUNK_SIZE];
    for _ in 0..SLOW_BODY_SIZE / SLOW_CHUNK_SIZE {
        match stream.write_all(&chunk).await {
            Ok(()) => tokio::time::sleep(Duration::from_millis(5)).await,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return;
            }
            Err(error) => panic!("unexpected pooled response write failure: {error}"),
        }
    }
    panic!("client reset did not cancel the pooled slow response");
}

async fn request_slow_response(frontend: SocketAddr, path: &str) -> Vec<u8> {
    let mut client = TcpStream::connect(frontend).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    client
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    read_response_with_body(&mut client, SLOW_BODY_SIZE)
        .await
        .expect("second slow response should complete from the pooled backend")
}

async fn request_probe(frontend: SocketAddr, path: &str) -> Vec<u8> {
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let response = read_to_end_within(&mut client, Duration::from_secs(2)).await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    body.to_vec()
}

async fn wait_for_pooled_connection(state: &SharedState) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.upstream_keepalive.len("p0_backend") == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("half-closed slow response should return its upstream connection to the pool");
}

async fn wait_for_no_pooled_connection(state: &SharedState) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.upstream_keepalive.len("p0_backend") != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("reset must drop the incomplete pooled upstream connection");
}

fn assert_complete_response(response: &[u8]) {
    let (headers, body) = split_http_response(response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), SLOW_BODY_SIZE);
    assert!(body.iter().all(|byte| *byte == b'x'));
}

async fn bind_backend() -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping mixed half-close/keepalive test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind mixed half-close/keepalive backend: {error}"),
    }
}
