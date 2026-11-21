use super::support::*;
use super::*;
use crate::config::TempPathConfig;

#[tokio::test]
async fn applies_upstream_header_buffer_size_to_proxy_responses() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy buffer-size test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0, "upstream request closed before headers");
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\nX-Long: {}\r\n\r\nok",
                "x".repeat(3 * 1024)
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
                        listen 127.0.0.1:18582;
                        location /small {{
                            proxy_pass http://{upstream};
                            proxy_buffer_size 1k;
                        }}
                        location /large {{
                            proxy_pass http://{upstream};
                            proxy_buffer_size 8k;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let small = request_body(
        &config,
        "GET /small HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(small.starts_with("HTTP/1.1 502 Bad Gateway\r\n"), "{small}");

    let large = request_body(
        &config,
        "GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(large.starts_with("HTTP/1.1 200 OK\r\n"), "{large}");
    assert!(large.ends_with("ok"), "{large}");
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn applies_upstream_header_buffer_size_to_cgi_responses() {
    let stdout = format!(
        "Status: 200 OK\r\nContent-Type: text/plain\r\nX-Long: {}\r\n\r\nok",
        "x".repeat(3 * 1024)
    );
    let error = match parse_cgi_response(stdout.into_bytes(), 1024) {
        Ok(_) => panic!("oversized CGI response headers should fail"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("header too large"));
}

#[test]
fn buffering_off_uses_memory_only() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18585;
                    location / {
                        proxy_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let options = UpstreamBodyBufferOptions::upstream(&context.proxy_buffering);
    assert_eq!(options.memory_limit, MAX_BODY_BYTES);
    assert!(options.temp_path.is_none());
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn maps_proxy_buffers_to_fixed_window_buffer_geometry() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18586;
                    location / {
                        proxy_buffers 3 4k;
                        proxy_busy_buffers_size 4k;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let options = UpstreamBodyBufferOptions::upstream(&context.proxy_buffering);
    assert_eq!(options.buffer_count, 3);
    assert_eq!(options.buffer_size, 4096);
    assert_eq!(options.memory_limit, 3 * 4096);
    assert_eq!(options.busy_limit, 4096);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn spills_cgi_bodies_to_temp_files_and_cleans_them_up() {
    let id = std::process::id();
    let temp_dir = std::env::temp_dir().join(format!("starling-cgi-buffer-{id}"));
    let (mut upstream, mut gateway) = tokio::io::duplex(1024);
    upstream
        .write_all(b"Status: 200 OK\r\nContent-Type: text/plain\r\n\r\nabcdefghijkl")
        .await
        .unwrap();
    upstream.shutdown().await.unwrap();
    let options = UpstreamBodyBufferOptions {
        memory_limit: 4,
        busy_limit: usize::MAX,
        buffer_count: 1,
        buffer_size: 4,
        temp_path: Some(TempPathConfig::new(temp_dir.clone())),
        max_temp_file_size: 64,
        write_size: 2,
    };

    let mut response = read_cgi_stream_with_policy(
        &mut gateway,
        0,
        4096,
        UpstreamBodyBufferPolicy::fixed(options),
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert!(matches!(
        &response.body,
        ResponseBody::FileWithPrefix { .. }
    ));
    let temp_file = response
        .body_temp_file
        .as_ref()
        .expect("temp file guard")
        .path()
        .to_path_buf();
    assert!(temp_file.exists());
    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    assert_eq!(body.into_memory(64).await.unwrap(), b"abcdefghijkl");
    drop(response);
    assert!(!temp_file.exists());
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn parses_cgi_headers_split_across_upstream_chunks() {
    let mut decoder = CgiResponseDecoder::with_policy(
        4096,
        UpstreamBodyBufferPolicy::fixed(UpstreamBodyBufferOptions::memory(MAX_BODY_BYTES)),
    );
    decoder
        .push(b"Status: 201 Created\r\nContent-Ty")
        .await
        .unwrap();
    decoder
        .push(b"pe: text/plain\r\n\r\nsplit-body")
        .await
        .unwrap();
    let mut response = decoder.finish().await.unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.header("Content-Type"), Some("text/plain"));
    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    assert_eq!(body.into_memory(64).await.unwrap(), b"split-body");
}

#[tokio::test]
async fn buffers_fastcgi_stdout_across_records() {
    let id = std::process::id();
    let temp_dir = std::env::temp_dir().join(format!("starling-fastcgi-buffer-{id}"));
    let (mut backend, mut gateway) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        write_fastcgi_record(&mut backend, FCGI_STDOUT, b"Status: 200 OK\r\nContent-Ty")
            .await
            .unwrap();
        write_fastcgi_record(
            &mut backend,
            FCGI_STDOUT,
            b"pe: text/plain\r\n\r\nabcdefghijkl",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut backend, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        backend.shutdown().await.unwrap();
    });
    let options = UpstreamBodyBufferOptions {
        memory_limit: 4,
        busy_limit: usize::MAX,
        buffer_count: 1,
        buffer_size: 4,
        temp_path: Some(TempPathConfig::new(temp_dir.clone())),
        max_temp_file_size: 64,
        write_size: 2,
    };

    let (mut response, reusable) = read_fastcgi_response_with_policy(
        &mut gateway,
        &[],
        0,
        4096,
        UpstreamBodyBufferPolicy::fixed(options),
    )
    .await
    .unwrap();
    assert!(reusable);
    assert_eq!(response.status, 200);
    assert!(matches!(
        &response.body,
        ResponseBody::FileWithPrefix { .. }
    ));
    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    assert_eq!(body.into_memory(64).await.unwrap(), b"abcdefghijkl");
    drop(response);
    let _ = std::fs::remove_dir_all(temp_dir);
    task.await.unwrap();
}

#[tokio::test]
async fn proxies_responses_larger_than_the_memory_buffers() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy spill test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "x".repeat(16 * 1024 + 17);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "upstream request closed before headers");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18583;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_file_write_size 4k;
                            proxy_temp_path proxy-temp;
                            proxy_max_temp_file_size 16k;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_bytes(
        &config,
        "GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(response_body, expected.as_bytes());
    let temp_dir = prefix.join("proxy-temp");
    assert!(
        !temp_dir.exists() || std::fs::read_dir(&temp_dir).unwrap().next().is_none(),
        "upstream temp file was not cleaned up"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn streams_proxy_responses_larger_than_the_temp_file_limit() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy temp limit test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "x".repeat(16 * 1024);
    let expected = body.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "upstream request closed before headers");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18584;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_file_write_size 4k;
                            proxy_temp_path proxy-temp;
                            proxy_max_temp_file_size 4k;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_bytes(
        &config,
        "GET /large HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(response_body, expected.as_bytes());
    let temp_dir = prefix.join("proxy-temp");
    assert!(
        !temp_dir.exists() || std::fs::read_dir(&temp_dir).unwrap().next().is_none(),
        "failed upstream temp file was not cleaned up"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn temp_file_failure_after_commit_closes_without_rewriting_the_response() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy temp failure test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let body = "x".repeat(16 * 1024);
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "upstream request closed before headers");
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18586;
                        location / {{
                            proxy_pass http://{upstream};
                            proxy_buffers 2 4k;
                            proxy_busy_buffers_size 4k;
                            proxy_temp_file_write_size 4k;
                            proxy_temp_path proxy-temp;
                            proxy_max_temp_file_size 16k;
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    std::fs::write(prefix.join("proxy-temp"), b"not a directory").unwrap();

    let response = request_bytes(
        &config,
        "GET /failure HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, response_body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        !headers.starts_with("HTTP/1.1 502 ") && !headers.starts_with("HTTP/1.1 504 "),
        "committed response must not be rewritten after a temp-file failure: {headers}"
    );
    assert!(
        response_body.len() < 16 * 1024,
        "temp-file failure should truncate the response after the buffered prefix"
    );
    assert!(
        !response_body.is_empty(),
        "the buffered prefix should be sent"
    );
    assert_eq!(
        std::fs::read(prefix.join("proxy-temp")).unwrap(),
        b"not a directory"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
