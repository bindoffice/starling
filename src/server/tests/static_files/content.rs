use super::*;

#[tokio::test]
async fn sendfile_serves_large_static_files_over_plain_tcp() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping sendfile runtime test: sockets not permitted");
            return;
        }
        Err(error) => panic!("bind sendfile listener: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen {address};
                    root html;
                    sendfile on;
                    location / {{ }}
                }}
            }}
        "#
    );
    let body = "starling-sendfile\n".repeat(16 * 1024);
    let (prefix, config) = test_config(&source, &[("html/blob.bin", &body)]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server = tokio::spawn(async move {
        let (stream, remote) = listener.accept().await.unwrap();
        let (stream, control) =
            ClientSocketControl::attach_tcp(stream).expect("attach socket control");
        handle_http_connection_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), ListenAddress::Inet(address)),
            group,
            state,
            None,
            control,
        )
        .await;
    });

    let mut client = TcpStream::connect(address).await.unwrap();
    client
        .write_all(b"GET /blob.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    server.await.unwrap();

    let (headers, received) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(
        headers.contains(&format!("Content-Length: {}\r\n", body.len())),
        "{headers}"
    );
    assert_eq!(received, body.as_bytes());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn open_file_cache_reuses_the_opened_file_between_revalidations() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                open_file_cache max=4 inactive=1h;
                open_file_cache_valid 1h;
                server {
                    listen 127.0.0.1:18151;
                    root html;
                    location / { }
                }
            }
        "#,
        &[("html/cached.txt", "first version\n")],
    );
    let request = "GET /cached.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body(&config, request).await;
    assert!(first.ends_with("first version\n"));

    std::fs::write(prefix.join("html/cached.txt"), "second version\n").unwrap();
    let second = request_body(&config, request).await;
    assert!(second.ends_with("first version\n"), "{second}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn serves_static_content_and_configuration_directives() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                error_log logs/error.log error;
                pid logs/nginx.pid;
                events { worker_connections 64; }
                http {
                    default_type application/octet-stream;
                    access_log logs/access.log;
                    server {
                        listen 127.0.0.1:18080;
                        server_name localhost;
                        root html;
                        index index.html;
                        location = /health {
                            add_header X-Health yes always;
                            return 200 "healthy\n";
                        }
                        location / {
                            try_files $uri $uri/ =404;
                        }
                    }
                }
            "#,
        &[
            ("html/index.html", "welcome\n"),
            ("html/hello.txt", "hello file\n"),
        ],
    );
    let body = request_body(
        &config,
        "GET /hello.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(body.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(body.contains("Content-Type: text/plain\r\n"));
    assert!(body.ends_with("hello file\n"));

    let body = request_body(
        &config,
        "GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(body.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(body.contains("X-Health: yes\r\n"));
    assert!(body.ends_with("healthy\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn serves_empty_gif_and_random_directory_indexes() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18139;
                        root html;
                        location = /pixel.gif {
                            empty_gif;
                        }
                        location /random/ {
                            random_index on;
                        }
                    }
                }
            "#,
        &[
            ("html/random/a.txt", "alpha\n"),
            ("html/random/b.txt", "bravo\n"),
            ("html/random/.hidden", "hidden\n"),
        ],
    );
    let pixel = request_bytes(
        &config,
        "GET /pixel.gif HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let header_end = pixel
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = std::str::from_utf8(&pixel[..header_end]).unwrap();
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(headers.contains("Content-Type: image/gif\r\n"));
    assert!(headers.contains("Content-Length: 43\r\n"));
    assert_eq!(pixel[header_end + 4..].len(), 43);
    assert!(pixel[header_end + 4..].starts_with(b"GIF89a"));

    let random = request_body(
        &config,
        "GET /random/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(random.starts_with("HTTP/1.1 200 OK\r\n"), "{random}");
    assert!(
        random.ends_with("alpha\n") || random.ends_with("bravo\n"),
        "{random}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_charset_to_configured_mime_types() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    charset utf-8;
                    charset_types text/plain application/json;
                    server {
                        listen 127.0.0.1:18144;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[("html/note.txt", "plain\n"), ("html/page.html", "html\n")],
    );
    let plain = request_body(
        &config,
        "GET /note.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(plain.contains("Content-Type: text/plain; charset=utf-8\r\n"));
    let html = request_body(
        &config,
        "GET /page.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(html.contains("Content-Type: text/html\r\n"));
    assert!(!html.contains("charset="));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn serves_uppercase_extensions_with_configured_and_builtin_mime_types() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    default_type application/octet-stream;
                    types {
                        application/x-configured mixed;
                    }
                    server {
                        listen 127.0.0.1:18145;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[
            ("html/PAGE.HTML", "html\n"),
            ("html/FILE.MIXED", "configured\n"),
        ],
    );
    for (uri, content_type) in [
        ("/PAGE.HTML", "text/html"),
        ("/FILE.MIXED", "application/x-configured"),
    ] {
        let response = request_body(
            &config,
            &format!("GET {uri} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.contains(&format!("Content-Type: {content_type}\r\n")),
            "{response}"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn suppresses_missing_file_access_logs_when_disabled() {
    let source = |setting: &str| {
        format!(
            r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    http {{
                        access_log logs/access.log;
                        server {{
                            listen 127.0.0.1:18145;
                            root html;
                            {setting}
                            location / {{ }}
                        }}
                    }}
                "#
        )
    };
    let (prefix, config) = test_config(&source("log_not_found off;"), &[]);
    request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let suppressed = std::fs::read_to_string(prefix.join("logs/access.log")).unwrap_or_default();
    assert!(suppressed.is_empty(), "{suppressed}");
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(&source(""), &[]);
    request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let logged = std::fs::read_to_string(prefix.join("logs/access.log")).unwrap();
    assert!(logged.contains("404"), "{logged}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn honors_merge_slashes_in_request_uris() {
    let source = |merge_slashes: &str| {
        format!(
            r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    http {{
                        server {{
                            listen 127.0.0.1:18140;
                            {merge_slashes}
                            location / {{ return 200 "$uri\n"; }}
                        }}
                    }}
                "#
        )
    };
    let (prefix, config) = test_config(&source("merge_slashes on;"), &[]);
    let merged = request_body(
        &config,
        "GET /a//b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(merged.ends_with("/a/b\n"), "{merged}");
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(&source("merge_slashes off;"), &[]);
    let preserved = request_body(
        &config,
        "GET /a//b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(preserved.ends_with("/a//b\n"), "{preserved}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn filters_underscore_headers_unless_enabled() {
    let source = |underscores: &str| {
        format!(
            r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    http {{
                        server {{
                            listen 127.0.0.1:18141;
                            {underscores}
                            location / {{ return 200 "$http_x_test_header\n"; }}
                        }}
                    }}
                "#
        )
    };
    let request =
        "GET / HTTP/1.1\r\nHost: localhost\r\nX_Test_Header: visible\r\nConnection: close\r\n\r\n";
    let (prefix, config) = test_config(&source(""), &[]);
    let filtered = request_body(&config, request).await;
    assert!(filtered.ends_with("\n"), "{filtered}");
    assert!(!filtered.contains("visible"), "{filtered}");
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(&source("underscores_in_headers on;"), &[]);
    let accepted = request_body(&config, request).await;
    assert!(accepted.ends_with("visible\n"), "{accepted}");
    let _ = std::fs::remove_dir_all(prefix);
}
