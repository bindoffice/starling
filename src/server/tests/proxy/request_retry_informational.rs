use super::super::support::*;
use super::super::*;
use super::request_retry::{bind_test_listener, serve_request_body};

const EARLY_HINTS: &[u8] =
    b"HTTP/1.1 103 Early Hints\r\nLink: </discarded.css>; rel=preload\r\n\r\n";

#[tokio::test]
async fn failed_attempt_informational_response_does_not_leak_into_retry() {
    let Some(first) = bind_test_listener("informational retry first upstream").await else {
        return;
    };
    let Some(second) = bind_test_listener("informational retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let mut failed = Vec::from(EARLY_HINTS);
    failed.extend_from_slice(
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    let first_backend = serve_request_body(first, Box::leak(failed.into_boxed_slice()));
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
                    early_hints 1;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18720;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
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
    assert!(!response.contains("103 Early Hints"), "{response}");
    assert!(!response.contains("discarded.css"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn informational_response_followed_by_truncation_is_discarded_before_retry() {
    let Some(first) = bind_test_listener("truncated informational first upstream").await else {
        return;
    };
    let Some(second) = bind_test_listener("truncated informational second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        let request = read_body(&mut stream).await;
        stream.write_all(EARLY_HINTS).await.unwrap();
        stream.shutdown().await.unwrap();
        request
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
                    early_hints 1;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18721;
                        location / {{
                            proxy_pass http://file_retry;
                            proxy_next_upstream invalid_header non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
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
    assert!(!response.contains("103 Early Hints"), "{response}");
    assert!(!response.contains("discarded.css"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_body(stream: &mut TcpStream) -> Vec<u8> {
    let mut reader = BufReader::new(stream);
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(!line.is_empty(), "request headers ended unexpectedly");
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
    body
}
