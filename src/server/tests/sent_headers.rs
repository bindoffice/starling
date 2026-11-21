use super::support::*;

#[tokio::test]
async fn exposes_protocol_generated_sent_header_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    log_format sent
                        '$sent_http_connection|$sent_http_keep_alive|$sent_http_transfer_encoding|$sent_http_content_length';
                    server {
                        listen 127.0.0.1:18496;
                        keepalive_timeout 30s 12s;
                        access_log logs/sent.log sent;
                        location / { return 200 "hello"; }
                    }
                }
            "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n",
    )
    .await;
    assert!(
        response.contains("Connection: keep-alive\r\n"),
        "{response}"
    );
    assert!(
        response.contains("Keep-Alive: timeout=12\r\n"),
        "{response}"
    );
    assert!(response.contains("Content-Length: 5\r\n"), "{response}");

    let log = std::fs::read_to_string(prefix.join("logs/sent.log")).unwrap_or_default();
    assert_eq!(log.trim_end(), "keep-alive|timeout=12||5");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn sent_variables_reflect_chunked_trailers() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    log_format sent
                        '$sent_http_connection|$sent_http_keep_alive|$sent_http_transfer_encoding|$sent_http_content_length';
                    server {
                        listen 127.0.0.1:18497;
                        access_log logs/sent.log sent;
                        location / {
                            add_trailer X-Done yes always;
                            return 200 "hello";
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
    assert!(
        response.contains("Transfer-Encoding: chunked\r\n"),
        "{response}"
    );
    assert!(!response.contains("Content-Length:"), "{response}");
    assert!(response.contains("Connection: close\r\n"), "{response}");
    assert!(!response.contains("Keep-Alive:"), "{response}");

    let log = std::fs::read_to_string(prefix.join("logs/sent.log")).unwrap_or_default();
    assert_eq!(log.trim_end(), "close||chunked|");
    let _ = std::fs::remove_dir_all(prefix);
}
