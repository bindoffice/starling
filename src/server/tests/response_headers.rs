use super::support::*;
use super::*;

#[tokio::test]
async fn exposes_upstream_and_sent_response_header_variables() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\n\
                  Content-Type: text/plain\r\n\
                  X-Visible: visible\r\n\
                  X-Hidden: hidden\r\n\
                  X-Multi: one\r\n\
                  X-Multi: two\r\n\
                  Content-Length: 2\r\n\
                  Connection: close\r\n\r\nok",
            )
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    log_format response_headers
                        '$sent_http_content_type|$sent_http_x_upstream_visible|$sent_http_x_added';
                    log_format upstream_headers
                        '$upstream_http_content_type|$upstream_http_x_visible|$upstream_http_x_hidden|$upstream_http_x_multi';
                    server {{
                        listen 127.0.0.1:18494;
                        access_log logs/response.log response_headers;
                        access_log logs/upstream.log upstream_headers;
                        location / {{
                            proxy_pass http://{upstream_address};
                            proxy_hide_header X-Hidden;
                            add_header X-Upstream-Visible $upstream_http_x_visible always;
                            add_header X-Added $sent_http_content_type always;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.contains("X-Upstream-Visible: visible\r\n"),
        "{response}"
    );
    assert!(response.contains("X-Added: text/plain\r\n"), "{response}");
    assert!(!response.contains("X-Hidden:"), "{response}");

    let response_log =
        std::fs::read_to_string(prefix.join("logs/response.log")).unwrap_or_default();
    assert_eq!(response_log.trim_end(), "text/plain|visible|text/plain");
    let upstream_log =
        std::fs::read_to_string(prefix.join("logs/upstream.log")).unwrap_or_default();
    assert_eq!(
        upstream_log.trim_end(),
        "text/plain|visible|hidden|one, two"
    );

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn omits_empty_add_header_values_like_nginx() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18499;
                    location / {
                        add_header X-Empty $request_body;
                        add_header X-Empty-Always $request_body always;
                        add_header ETag original;
                        add_header ETag "";
                        add_header X-Present present;
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!response.contains("X-Empty:"), "{response}");
    assert!(!response.contains("X-Empty-Always:"), "{response}");
    assert!(!response.contains("ETag:"), "{response}");
    assert!(response.contains("X-Present: present\r\n"), "{response}");

    let _ = std::fs::remove_dir_all(prefix);
}
