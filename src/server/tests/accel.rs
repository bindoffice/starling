use super::support::*;
use super::*;

#[test]
fn captures_upstream_accel_rate_and_charset_controls() {
    let mut response = Response::bytes(200, "text/plain; charset=iso-8859-1", "body");
    response.add_header("X-Accel-Limit-Rate", "4096");
    response.add_header("X-Accel-Limit-Rate", "8192");
    response.add_header("X-Accel-Charset", "utf-8");
    response.add_header("X-Accel-Redirect", "/internal");

    capture_upstream_control(&mut response, &[]);
    let control = response.upstream_control().unwrap();
    assert_eq!(control.redirect.as_deref(), Some("/internal"));
    assert_eq!(control.limit_rate, Some(4096));
    assert_eq!(control.charset.as_deref(), Some("utf-8"));
    assert!(response.header("X-Accel-Redirect").is_none());

    apply_upstream_charset(&mut response);
    assert_eq!(
        response.header("Content-Type"),
        Some("text/plain; charset=utf-8")
    );
    let rate = response_rate_with_upstream(
        ResponseRate {
            rate: 1024,
            after: 128,
        },
        &response,
    );
    assert_eq!(rate.rate, 4096);
    assert_eq!(rate.after, 128);
}

#[test]
fn ignored_accel_headers_do_not_change_response_behavior() {
    let ignore = [
        "X-Accel-Redirect".to_string(),
        "X-Accel-Limit-Rate".to_string(),
        "X-Accel-Charset".to_string(),
    ];
    let mut response = Response::bytes(200, "text/plain", "body");
    response.add_header("X-Accel-Limit-Rate", "1");
    response.add_header("X-Accel-Charset", "utf-8");
    response.add_header("X-Accel-Redirect", "/internal");

    capture_upstream_control(&mut response, &ignore);
    let control = response.upstream_control().unwrap();
    assert!(control.redirect.is_none());
    assert!(control.limit_rate.is_none());
    assert!(control.charset.is_none());
    assert_eq!(control.headers.len(), response.headers.len());
    assert_eq!(response.header("X-Accel-Limit-Rate"), Some("1"));
    assert_eq!(response.header("X-Accel-Charset"), Some("utf-8"));
    assert_eq!(response.header("X-Accel-Redirect"), Some("/internal"));
}

#[tokio::test]
async fn applies_proxy_accel_charset_and_limit_rate() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let limited_body = "x".repeat(2048);
    let responses = vec![
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\n\
         X-Accel-Charset: utf-8\r\n\r\ncharset"
            .to_string(),
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 7\r\n\
         X-Accel-Charset: utf-8\r\n\r\nignored"
            .to_string(),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
             X-Accel-Limit-Rate: 2048\r\n\r\n{limited_body}",
            limited_body.len()
        ),
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
             X-Accel-Limit-Rate: 2048\r\n\r\n{limited_body}",
            limited_body.len()
        ),
    ];
    let backend = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18145;
                    location = /charset {{
                        proxy_pass http://{backend_address};
                    }}
                    location = /ignored-charset {{
                        proxy_pass http://{backend_address};
                        proxy_ignore_headers X-Accel-Charset;
                    }}
                    location = /rate {{
                        proxy_pass http://{backend_address};
                    }}
                    location = /ignored-rate {{
                        proxy_pass http://{backend_address};
                        proxy_ignore_headers X-Accel-Limit-Rate;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);

    let charset = request_body(
        &config,
        "GET /charset HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        charset.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{charset}"
    );
    assert!(!charset.contains("X-Accel-Charset:"), "{charset}");

    let ignored_charset = request_body(
        &config,
        "GET /ignored-charset HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        ignored_charset.contains("Content-Type: text/plain\r\n"),
        "{ignored_charset}"
    );
    assert!(!ignored_charset.contains("charset="), "{ignored_charset}");

    let started = Instant::now();
    let limited = request_body(
        &config,
        "GET /rate HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(started.elapsed() >= Duration::from_millis(900), "{limited}");
    assert!(limited.ends_with(&limited_body), "{limited}");

    let ignored_rate = request_body(
        &config,
        "GET /ignored-rate HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(ignored_rate.ends_with(&limited_body), "{ignored_rate}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
