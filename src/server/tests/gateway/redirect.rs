use super::*;

#[tokio::test]
async fn follows_fastcgi_accel_redirects_and_honors_ignore_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for response in [
            "Status: 200 OK\r\n\
             Content-Type: text/html\r\n\
             X-Accel-Redirect: /internal/file.txt\r\n\r\nbackend-redirect",
            "Status: 200 OK\r\n\
             Content-Type: text/plain\r\n\
             X-Accel-Redirect: /internal/file.txt\r\n\r\nignored-backend",
            "Status: 200 OK\r\n\
             Content-Type: text/html\r\n\
             X-Accel-Redirect: @fallback\r\n\r\nbackend-named",
            "Status: 200 OK\r\n\
             Content-Type: text/html\r\n\
             X-Accel-Redirect: @missing\r\n\r\nbackend-missing",
        ]
        .into_iter()
        {
            let (mut stream, _) = listener.accept().await.unwrap();
            loop {
                let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
                if record_type == FCGI_STDIN && content.is_empty() {
                    break;
                }
            }
            write_fastcgi_record(&mut stream, FCGI_STDOUT, response.as_bytes())
                .await
                .unwrap();
            write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
                .await
                .unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18142;
                    root html;
                    location = /redirect {{
                        fastcgi_pass {backend_address};
                    }}
                    location = /ignored {{
                        fastcgi_pass {backend_address};
                        fastcgi_ignore_headers X-Accel-Redirect;
                    }}
                    location = /named {{
                        fastcgi_pass {backend_address};
                    }}
                    location = /invalid-named {{
                        fastcgi_pass {backend_address};
                    }}
                    location /internal/ {{
                        internal;
                    }}
                    location @fallback {{
                        return 200 "$request_method\n";
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[("html/internal/file.txt", "internal-file\n")]);

    let redirected = request_body(
        &config,
        "GET /redirect HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
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

    let named = request_body(
        &config,
        "POST /named HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(named.starts_with("HTTP/1.1 200 OK\r\n"), "{named}");
    assert!(named.ends_with("POST\n"), "{named}");

    let invalid_named = request_body(
        &config,
        "GET /invalid-named HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        invalid_named.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{invalid_named}"
    );

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
