use super::*;

#[tokio::test]
async fn truncated_upstream_body_fails_before_a_buffering_filter_commits() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping truncated sub_filter test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
        }
        reader
            .get_mut()
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 20\r\n\
                  Connection: close\r\n\r\nHello",
            )
            .await
            .unwrap();
        reader.get_mut().shutdown().await.unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18726;
                        location / {{
                            proxy_pass http://{upstream_address};
                            sub_filter_types text/plain;
                            sub_filter Hello Hi;
                            proxy_next_upstream off;
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
    backend.await.unwrap();
    assert!(
        response.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{response}"
    );
    assert!(!response.ends_with("Hello"), "{response}");
    assert!(!response.contains("\r\n\r\nHi"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_sub_filter_replacements_and_mime_rules() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18143;
                        location /once {
                            sub_filter_types text/plain;
                            sub_filter Hello Hi;
                            return 200 "Hello Hello";
                        }
                        location /all {
                            sub_filter_types text/plain;
                            sub_filter Hello Hi;
                            sub_filter_once off;
                            return 200 "Hello Hello";
                        }
                        location /vars {
                            sub_filter_types text/plain;
                            sub_filter Hello "$host";
                            return 200 "Hello";
                        }
                        location /skip {
                            sub_filter Hello Hi;
                            return 200 "Hello";
                        }
                    }
                }
            "#,
        &[],
    );
    let request =
        |path: &str| format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    let once = request_body(&config, &request("/once")).await;
    assert!(once.ends_with("Hi Hello"), "{once}");
    let all = request_body(&config, &request("/all")).await;
    assert!(all.ends_with("Hi Hi"), "{all}");
    let vars = request_body(&config, &request("/vars")).await;
    assert!(vars.ends_with("localhost"), "{vars}");
    let skip = request_body(&config, &request("/skip")).await;
    assert!(skip.ends_with("Hello"), "{skip}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_gzip_proxied_rules_to_upstream_responses() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping proxied gzip integration test: binding TCP sockets is not permitted"
            );
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            loop {
                line.clear();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let body = "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
        }
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    gzip on;
                    gzip_types text/plain;
                    gzip_min_length 1;
                    server {{
                        listen 127.0.0.1:18137;
                        location /off {{
                            proxy_pass http://{upstream_address};
                        }}
                        location /on {{
                            proxy_pass http://{upstream_address};
                            gzip_proxied any;
                        }}
                    }}
                }}
            "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let off = request_bytes(
            &config,
            "GET /off HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await;
    let on = request_bytes(
        &config,
        "GET /on HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    let headers = |response: &[u8]| {
        let end = response
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        String::from_utf8_lossy(&response[..end]).into_owned()
    };
    assert!(!headers(&off).contains("Content-Encoding: gzip"));
    assert!(headers(&on).contains("Content-Encoding: gzip"));
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
