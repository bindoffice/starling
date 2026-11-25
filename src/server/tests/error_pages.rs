use super::support::*;
use crate::server::{Header, Request, internal_redirect_request};

#[test]
fn follows_recursive_error_pages_only_when_enabled() {
    run_with_worker_stack(async {
        let (prefix, config) = test_config(
            r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18520 default_server;
                    server_name disabled.test;
                    root html;
                    error_page 404 /hop;
                    location = /hop {
                        internal;
                        recursive_error_pages on;
                        error_page 404 /final;
                        try_files $uri =404;
                    }
                    location = /final { internal; }
                    location / { try_files $uri =404; }
                }
                server {
                    listen 127.0.0.1:18520;
                    server_name enabled.test;
                    root html;
                    recursive_error_pages on;
                    error_page 404 /hop;
                    location = /hop {
                        internal;
                        error_page 404 /final;
                        try_files $uri =404;
                    }
                    location = /final { internal; }
                    location / { try_files $uri =404; }
                }
            }
        "#,
            &[("html/final", "redirected page\n")],
        );

        let disabled = request_body(
            &config,
            "GET /missing HTTP/1.1\r\nHost: disabled.test\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(disabled.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(disabled.contains("<center><h1>404 Not Found</h1></center>"));
        assert!(!disabled.contains("redirected page"));

        let enabled = request_body(
            &config,
            "GET /missing HTTP/1.1\r\nHost: enabled.test\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(enabled.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(enabled.ends_with("redirected page\n"));
        let _ = std::fs::remove_dir_all(prefix);
    });
}

#[tokio::test]
async fn location_error_pages_replace_server_inheritance() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18521;
                    root html;
                    error_page 404 /server-fallback;
                    location /replaced {
                        error_page 500 /server-fallback;
                        try_files $uri =404;
                    }
                    location = /server-fallback { internal; }
                    location / { try_files $uri =404; }
                }
            }
        "#,
        &[("html/server-fallback", "server fallback\n")],
    );

    let response = request_body(
        &config,
        "GET /replaced HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(response.contains("<center><h1>404 Not Found</h1></center>"));
    assert!(!response.contains("server fallback"));

    let inherited = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(inherited.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(inherited.ends_with("server fallback\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn internal_redirect_keeps_buffered_body_but_discards_live_body() {
    let mut request = Request {
        method: "POST".into(),
        target: "/proxy".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "7".into(),
            },
        ])
        .into(),
        body: b"payload".to_vec(),
        body_stream: None,
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18522".into(),
        keep_alive: false,
    };

    let redirected = internal_redirect_request(&request);
    assert_eq!(redirected.method, "GET");
    assert_eq!(redirected.body, b"payload");
    assert!(
        redirected
            .headers
            .iter()
            .all(|header| !header.name.eq_ignore_ascii_case("content-length"))
    );

    let (_, stream) = crate::http::RequestBodyStream::channel();
    request.body.clear();
    request.body_stream = Some(stream);
    let redirected = internal_redirect_request(&request);
    assert!(redirected.body.is_empty());
    assert!(redirected.body_stream.is_none());
}
