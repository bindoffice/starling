use super::super::support::*;
use super::super::*;

#[tokio::test]
async fn named_upstream_retries_file_backed_request_body_from_the_start() {
    let first = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy request retry test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind first upstream: {error}"),
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = serve_request_body(
        first,
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let second_backend = serve_request_body(
        second,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18580;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body should be removed after retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_retries_file_backed_request_body_after_connect_failure() {
    let unavailable = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy connect retry test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind unavailable upstream: {error}"),
    };
    let unavailable_address = unavailable.local_addr().unwrap();
    drop(unavailable);

    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second_address = second.local_addr().unwrap();
    let second_backend = serve_request_body(
        second,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {unavailable_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18581;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream error timeout non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body should be removed after connect retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_rewinds_file_backed_request_body_after_partial_write() {
    let first = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy partial-write retry test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind first upstream: {error}"),
    };
    let second = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy partial-write retry test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind second upstream: {error}"),
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        loop {
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert!(!line.is_empty(), "request headers ended unexpectedly");
            if line == "\r\n" {
                break;
            }
        }
        let mut prefix = [0_u8; 4];
        reader.read_exact(&mut prefix).await.unwrap();
        assert_eq!(&prefix, b"part");
        drop(reader);
        stream.shutdown().await.unwrap();
        prefix.to_vec()
    });
    let second_backend = serve_request_body(
        second,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18582;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream error timeout non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"part");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body should be removed after rewind retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_retries_file_backed_request_body_after_read_timeout() {
    let Some(first) = bind_test_listener("proxy read-timeout retry first upstream").await else {
        return;
    };
    let Some(second) = bind_test_listener("proxy read-timeout retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = read_backend_body_then_delay(first, Duration::from_millis(200));
    let second_backend = serve_request_body(
        second,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18600;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_read_timeout 20ms;
                            proxy_next_upstream timeout non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body should be removed after timeout retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_retries_file_backed_request_body_after_invalid_response() {
    let Some(first) = bind_test_listener("proxy invalid-response retry first upstream").await
    else {
        return;
    };
    let Some(second) = bind_test_listener("proxy invalid-response retry second upstream").await
    else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = serve_request_body(first, b"not a valid HTTP response\r\n\r\n");
    let second_backend = serve_request_body(
        second,
        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18601;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream invalid_header non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body should be removed after invalid-response retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

pub(super) async fn bind_test_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

fn read_backend_body_then_delay(
    listener: TcpListener,
    delay: Duration,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                content_length = Some(value.parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0_u8; content_length.expect("request Content-Length")];
        reader.read_exact(&mut body).await.unwrap();
        tokio::time::sleep(delay).await;
        body
    })
}

pub(super) fn serve_request_body(
    listener: TcpListener,
    response: &'static [u8],
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut content_length = None;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line
                .split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim())
            {
                content_length = Some(value.parse::<usize>().unwrap());
            }
        }
        let mut body = vec![0_u8; content_length.expect("request Content-Length")];
        reader.read_exact(&mut body).await.unwrap();
        reader.get_mut().write_all(response).await.unwrap();
        reader.get_mut().shutdown().await.unwrap();
        body
    })
}
