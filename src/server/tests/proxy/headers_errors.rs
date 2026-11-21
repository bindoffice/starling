use super::*;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

#[test]
fn filters_upstream_headers_with_hide_and_pass_rules() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18098;
                        proxy_hide_header X-Parent;
                        location /inherit/ {
                            proxy_pass http://127.0.0.1:19000;
                        }
                        location /override/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_hide_header Server;
                            proxy_hide_header X-Local;
                            proxy_pass_header Server;
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let headers = || {
        [
            ("Date", "upstream"),
            ("Server", "upstream"),
            ("X-Pad", "pad"),
            ("X-Accel-Redirect", "/internal"),
            ("X-Parent", "parent"),
            ("X-Local", "local"),
            ("X-Keep", "keep"),
            ("Connection", "close"),
        ]
        .into_iter()
        .map(|(name, value)| Header {
            name: name.into(),
            value: value.into(),
        })
        .collect::<Vec<_>>()
    };

    let inherited = filter_proxy_response_headers(headers(), &server.locations[0].context);
    let inherited_names: Vec<_> = inherited
        .iter()
        .map(|header| header.name.as_str())
        .collect();
    assert_eq!(inherited_names, vec!["X-Local", "X-Keep"]);

    let overridden = filter_proxy_response_headers(headers(), &server.locations[1].context);
    let overridden_names: Vec<_> = overridden
        .iter()
        .map(|header| header.name.as_str())
        .collect();
    assert_eq!(overridden_names, vec!["Server", "X-Parent", "X-Keep"]);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn gates_proxy_error_pages_with_proxy_intercept_errors() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18100;
                        location /passthrough/ {
                            proxy_pass http://127.0.0.1:19000;
                            error_page 502 /fallback;
                        }
                        location /intercept/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_intercept_errors on;
                            error_page 502 /fallback;
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(!should_intercept_error_pages(
        true,
        &server.locations[0].context
    ));
    assert!(should_intercept_error_pages(
        true,
        &server.locations[1].context
    ));
    assert!(should_intercept_error_pages(
        false,
        &server.locations[0].context
    ));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn upstream_3xx_only_disables_streaming_when_an_error_page_intercepts_it() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18111;
                        location /matching/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_http_version 1.1;
                            proxy_buffering off;
                            proxy_intercept_errors on;
                            error_page 302 /fallback;
                        }
                        location /no-match/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_http_version 1.1;
                            proxy_buffering off;
                            proxy_intercept_errors on;
                            error_page 404 /fallback;
                        }
                        location /intercept-off/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_http_version 1.1;
                            proxy_buffering off;
                            proxy_intercept_errors off;
                            error_page 302 /fallback;
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];

    assert!(!can_stream_response(&server.locations[0].context, 302, &[]));
    assert!(can_stream_response(&server.locations[1].context, 302, &[]));
    assert!(can_stream_response(&server.locations[2].context, 302, &[]));
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn intercepts_upstream_401_and_preserves_www_authenticate_challenges() {
    let socket_path = test_socket_path("proxy-intercept-401");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_head(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\n\
                      Content-Type: text/plain\r\n\
                      Content-Length: 13\r\n\
                      WWW-Authenticate: Basic realm=\"one\"\r\n\
                      WWW-Authenticate: Bearer realm=\"two\"\r\n\
                      Connection: close\r\n\r\n\
                      upstream-401\n",
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        }
    });

    let socket = socket_path.display().to_string();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18101;
                        root html;
                        error_page 401 /auth-error;
                        location = /auth-error {{ internal; }}
                        location /intercept/ {{
                            proxy_pass http://unix:{socket};
                            proxy_intercept_errors on;
                        }}
                        location /passthrough/ {{
                            proxy_pass http://unix:{socket};
                            proxy_intercept_errors off;
                        }}
                    }}
                }}
            "#
        ),
        &[("html/auth-error", "intercepted-401\n")],
    );

    let intercepted = request_body(
        &config,
        "GET /intercept/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        intercepted.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "{intercepted}"
    );
    assert!(intercepted.ends_with("intercepted-401\n"), "{intercepted}");
    assert!(!intercepted.contains("upstream-401"), "{intercepted}");
    assert_eq!(
        intercepted.matches("WWW-Authenticate:").count(),
        2,
        "{intercepted}"
    );

    let passed_through = request_body(
        &config,
        "GET /passthrough/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        passed_through.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "{passed_through}"
    );
    assert!(
        passed_through.ends_with("upstream-401\n"),
        "{passed_through}"
    );
    assert!(
        !passed_through.contains("intercepted-401"),
        "{passed_through}"
    );
    assert_eq!(
        passed_through.matches("WWW-Authenticate:").count(),
        2,
        "{passed_through}"
    );

    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn intercepts_upstream_errors_before_following_accel_redirects() {
    let socket_path = test_socket_path("proxy-error-interception");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request_head(&mut stream).await;
            stream
                .write_all(
                    b"HTTP/1.1 404 Not Found\r\n\
                      Content-Type: text/plain\r\n\
                      Content-Length: 13\r\n\
                      X-Accel-Redirect: /accel-target\r\n\
                      Connection: close\r\n\r\n\
                      upstream-404\n",
                )
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        }
    });

    let socket = socket_path.display().to_string();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18109;
                        root html;
                        error_page 404 /error-page;
                        location = /error-page {{ internal; }}
                        location = /accel-target {{
                            internal;
                            return 200 "accel-target\n";
                        }}
                        location /intercept/ {{
                            proxy_pass http://unix:{socket};
                            proxy_intercept_errors on;
                        }}
                        location /passthrough/ {{
                            proxy_pass http://unix:{socket};
                            proxy_intercept_errors off;
                        }}
                        location /no-page/ {{
                            proxy_pass http://unix:{socket};
                            proxy_intercept_errors on;
                            error_page 500 /error-page;
                        }}
                    }}
                }}
            "#
        ),
        &[("html/error-page", "intercepted-page\n")],
    );

    let intercepted = request_body(
        &config,
        "GET /intercept/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        intercepted.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{intercepted}"
    );
    assert!(intercepted.ends_with("intercepted-page\n"), "{intercepted}");

    for path in ["/passthrough/missing", "/no-page/missing"] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{path}: {response}"
        );
        assert!(response.ends_with("accel-target\n"), "{path}: {response}");
    }

    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_request_head<S>(stream: &mut S)
where
    S: AsyncRead + Unpin,
{
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await.unwrap();
        if read == 0 || line == "\r\n" {
            break;
        }
    }
}
