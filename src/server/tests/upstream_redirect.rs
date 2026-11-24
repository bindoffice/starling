use super::support::*;
use super::*;

#[tokio::test]
async fn follows_proxy_accel_redirects_and_honors_ignore_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for body in ["backend-redirect", "ignored-backend"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\
                 X-Accel-Redirect: /internal/file.txt\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18143;
                    root html;
                    location = /proxy {{
                        proxy_pass http://{backend_address};
                    }}
                    location = /ignored {{
                        proxy_pass http://{backend_address};
                        proxy_ignore_headers X-Accel-Redirect;
                    }}
                    location /internal/ {{
                        internal;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[("html/internal/file.txt", "internal-file\n")]);

    let redirected = request_body(
        &config,
        "GET /proxy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        redirected.starts_with("HTTP/1.1 200 OK\r\n"),
        "{redirected}"
    );
    assert!(!redirected.contains("X-Accel-Redirect:"), "{redirected}");
    assert!(redirected.ends_with("internal-file\n"), "{redirected}");

    let ignored = request_body(
        &config,
        "GET /ignored HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(ignored.starts_with("HTTP/1.1 200 OK\r\n"), "{ignored}");
    assert!(!ignored.contains("X-Accel-Redirect:"), "{ignored}");
    assert!(ignored.ends_with("ignored-backend"), "{ignored}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn follows_accel_redirect_without_waiting_for_the_upstream_body() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\n\
                  Content-Type: application/octet-stream\r\n\
                  Content-Length: 1048576\r\n\
                  X-Accel-Redirect: /internal/file.txt\r\n\r\n",
            )
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18233;
                    root html;
                    location = /proxy {{
                        proxy_pass http://{backend_address};
                    }}
                    location /internal/ {{
                        internal;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[("html/internal/file.txt", "internal-file\n")]);

    let response = tokio::time::timeout(
        Duration::from_millis(500),
        request_body(
            &config,
            "GET /proxy HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        ),
    )
    .await
    .expect("X-Accel-Redirect must not wait for the unread upstream body");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("internal-file\n"), "{response}");

    backend.abort();
    let _ = std::fs::remove_dir_all(prefix);
}
