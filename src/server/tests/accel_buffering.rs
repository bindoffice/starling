use super::support::*;
use super::*;

#[test]
fn captures_accel_buffering_last_valid_value() {
    let mut response = Response::bytes(200, "text/plain", "body");
    response.add_header("X-Accel-Buffering", "no");
    response.add_header("X-Accel-Buffering", "invalid");
    response.add_header("X-Accel-Buffering", "YES");

    capture_upstream_control(&mut response, &[]);
    assert_eq!(
        response
            .upstream_control()
            .and_then(|control| control.buffering),
        Some(AccelBuffering::Yes)
    );
}

#[tokio::test]
async fn proxy_honors_accel_buffering_and_ignore_headers() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy accel-buffering test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "x".repeat(16 * 1024);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        for buffering in ["no", "no", "yes"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
                 X-Accel-Buffering: {buffering}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18146;
                        location = /no {{
                            proxy_pass http://{upstream};
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_path temp-blocker 1;
                        }}
                        location = /ignored {{
                            proxy_pass http://{upstream};
                            proxy_buffers 6 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_path temp-blocker 1;
                            proxy_ignore_headers X-Accel-Buffering;
                        }}
                        location = /yes {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_path temp-blocker 1;
                        }}
                    }}
                }}
            "#
        ),
        &[("temp-blocker", "not a directory")],
    );

    let unbuffered = request_body(
        &config,
        "GET /no HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        unbuffered.starts_with("HTTP/1.1 200 OK\r\n"),
        "{unbuffered}"
    );
    assert!(unbuffered.ends_with(&expected), "{unbuffered}");
    assert!(!unbuffered.contains("X-Accel-Buffering:"), "{unbuffered}");

    let ignored = request_body(
        &config,
        "GET /ignored HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(ignored.starts_with("HTTP/1.1 200 OK\r\n"), "{ignored}");
    let (_, ignored_body) = split_http_response(ignored.as_bytes());
    assert_eq!(
        ignored_body.len(),
        expected.len(),
        "buffered streaming body was truncated"
    );
    assert!(ignored_body.iter().all(|byte| *byte == b'x'));

    let backend_response = request_body(
        &config,
        "GET /yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        backend_response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{backend_response}"
    );

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn ignored_accel_headers_do_not_block_streaming() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping ignored-accel streaming test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "z".repeat(16 * 1024);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        for header in [
            "X-Accel-Redirect: /internal",
            "X-Accel-Charset: utf-8",
            "X-Accel-Charset: ",
        ] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
                 {header}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18146;
                        location = /ignored-redirect {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_path temp-blocker 1;
                            proxy_ignore_headers X-Accel-Redirect;
                        }}
                        location = /ignored-charset {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_path temp-blocker 1;
                            proxy_ignore_headers X-Accel-Charset;
                        }}
                        location = /empty-charset {{
                            proxy_pass http://{upstream};
                            proxy_buffering off;
                            proxy_temp_path temp-blocker 1;
                        }}
                    }}
                }}
            "#
        ),
        &[("temp-blocker", "not a directory")],
    );

    for path in ["/ignored-redirect", "/ignored-charset", "/empty-charset"] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{path}: {response}"
        );
        assert!(
            response.ends_with(&expected),
            "{path}: response len {}, expected {}",
            response.len(),
            expected.len()
        );
    }

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
