use super::support::*;
use super::*;

#[tokio::test]
async fn stores_request_body_files_and_exposes_nginx_variables() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping request body file test: binding TCP sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..6 {
            let (mut stream, _) = upstream.accept().await.unwrap();
            tokio::spawn(async move {
                let (request, body) = read_backend_request(&mut stream).await;
                assert!(request.starts_with("POST /"), "{request}");
                let response_body = format!("ok:{body}");
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_body_temp_path body_temp 1 2;
                server {{
                    listen 127.0.0.1:18493;
                    location /on {{
                        client_body_in_file_only on;
                        proxy_pass http://{upstream_address};
                        add_header X-Request-Body-File $request_body_file always;
                        add_header X-Request-Body $request_body always;
                    }}
                    location /clean {{
                        client_body_in_file_only clean;
                        proxy_pass http://{upstream_address};
                        add_header X-Request-Body-File $request_body_file always;
                    }}
                    location /overflow {{
                        client_body_buffer_size 4;
                        proxy_pass http://{upstream_address};
                        add_header X-Request-Body-File $request_body_file always;
                        add_header X-Request-Body $request_body always;
                    }}
                    location /memory {{
                        client_body_buffer_size 8;
                        proxy_pass http://{upstream_address};
                        add_header X-Request-Body-File $request_body_file always;
                        add_header X-Request-Body $request_body always;
                    }}
                    location /unbuffered {{
                        client_body_in_file_only on;
                        proxy_request_buffering off;
                        proxy_pass http://{upstream_address};
                        add_header X-Request-Body-File $request_body_file always;
                        add_header X-Request-Body $request_body always;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);

    let persistent = request_body(
        &config,
        "POST /on HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    assert!(persistent.ends_with("ok:hello"));
    assert!(
        response_header(&persistent, "X-Request-Body")
            .unwrap_or_default()
            .is_empty()
    );
    let persistent_file = response_header(&persistent, "X-Request-Body-File")
        .expect("persistent request body file header");
    let persistent_path = PathBuf::from(persistent_file);
    assert_eq!(std::fs::read(&persistent_path).unwrap(), b"hello");
    let relative = persistent_path
        .strip_prefix(prefix.join("body_temp"))
        .expect("hashed client body temp path");
    assert_eq!(relative.components().count(), 3);

    let cleaned = request_body(
        &config,
        "POST /clean HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    let cleaned_file =
        response_header(&cleaned, "X-Request-Body-File").expect("cleaned request body file header");
    assert!(!Path::new(cleaned_file).exists());

    let overflow = request_body(
        &config,
        "POST /overflow HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    assert!(overflow.ends_with("ok:hello"));
    assert!(
        response_header(&overflow, "X-Request-Body")
            .unwrap_or_default()
            .is_empty()
    );
    let overflow_file = response_header(&overflow, "X-Request-Body-File")
        .expect("overflow request body file header");
    assert!(!Path::new(overflow_file).exists());

    let memory = request_body(
        &config,
        "POST /memory HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    assert!(memory.ends_with("ok:hello"));
    assert_eq!(response_header(&memory, "X-Request-Body"), Some("hello"));
    assert!(
        response_header(&memory, "X-Request-Body-File")
            .unwrap_or_default()
            .is_empty()
    );

    let unbuffered = request_body(
        &config,
        "POST /unbuffered HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\
         Connection: close\r\n\r\nhello",
    )
    .await;
    assert!(unbuffered.ends_with("ok:hello"));
    assert_eq!(
        response_header(&unbuffered, "X-Request-Body"),
        Some("hello")
    );
    assert!(
        response_header(&unbuffered, "X-Request-Body-File")
            .unwrap_or_default()
            .is_empty()
    );

    let empty = request_body(
        &config,
        "POST /on HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let empty_file =
        response_header(&empty, "X-Request-Body-File").expect("empty request body file header");
    assert_eq!(std::fs::read(empty_file).unwrap(), b"");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn temp_file_creation_failure_maps_to_500_for_spill_and_file_only() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18591;
                    location /spill {
                        return 200 "spill";
                    }
                    location /file-only {
                        client_body_in_file_only on;
                        return 200 "file";
                    }
                }
            }
        "#,
        &[],
    );
    let temp_path = prefix.join("body_temp");
    std::fs::write(&temp_path, b"not a directory").unwrap();

    let spilled = request_body(
        &config,
        "POST /spill HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;
    assert!(spilled.starts_with("HTTP/1.1 500 "), "{spilled}");

    let file_only = request_body(
        &config,
        "POST /file-only HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\
         Connection: close\r\n\r\non",
    )
    .await;
    assert!(file_only.starts_with("HTTP/1.1 500 "), "{file_only}");
    assert_eq!(
        std::fs::read(temp_path).unwrap(),
        b"not a directory",
        "request body temp path must not be replaced"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn file_only_lifecycle_is_preserved_or_cleaned_after_upstream_failure() {
    let missing_socket = test_socket_path("request-body-missing-upstream");
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:18592;
                        location /on {{
                            client_body_in_file_only on;
                            proxy_pass http://unix:{};
                            add_header X-Request-Body-File $request_body_file always;
                        }}
                        location /clean {{
                            client_body_in_file_only clean;
                            proxy_pass http://unix:{};
                            add_header X-Request-Body-File $request_body_file always;
                        }}
                    }}
                }}
            "#,
            missing_socket.display(),
            missing_socket.display()
        ),
        &[],
    );

    for (path, retained) in [("/on", true), ("/clean", false)] {
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\
             Connection: close\r\n\r\nhello"
        );
        let response = request_body(&config, &request).await;
        assert!(response.starts_with("HTTP/1.1 502 "), "{path}: {response}");
        let body_file = response_header(&response, "X-Request-Body-File")
            .unwrap_or_else(|| panic!("{path}: missing request body file header"));
        let body_file = Path::new(body_file);
        assert_eq!(
            body_file.exists(),
            retained,
            "{path}: {}",
            body_file.display()
        );
        if retained {
            assert_eq!(std::fs::read(body_file).unwrap(), b"hello");
            std::fs::remove_file(body_file).unwrap();
        }
    }

    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "failed upstreams must not leak request body files"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn incomplete_spooled_request_body_is_400_and_removes_the_partial_file() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 2;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18594;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let state = test_state(&config);
    let remote: SocketAddr = "127.0.0.1:45680".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(4096);
    let task = tokio::spawn(async move {
        handle_io(
            server,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
        )
        .await;
    });
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    task.await.unwrap();
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "an incomplete request body must remove its spool file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn client_reset_during_spool_removes_the_partial_file() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping canceled spool test: binding TCP is not permitted");
            return;
        }
        Err(error) => panic!("bind canceled spool listener: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 2;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18660;
                    location / { return 204; }
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server = tokio::spawn(async move {
        let (stream, remote) = listener.accept().await.unwrap();
        handle_io(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), ListenAddress::Inet(address)),
            group,
            state,
        )
        .await;
    });

    let mut client = TcpStream::connect(address).await.unwrap();
    let mut request = b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1048576\r\n\
                        Connection: close\r\n\r\n"
        .to_vec();
    request.extend(std::iter::repeat_n(b'x', 16 * 1024));
    client.write_all(&request).await.unwrap();

    let temp_root = prefix.join("body_temp");
    let mut observed = false;
    for _ in 0..200 {
        if contains_generated_temp_file(&temp_root) {
            observed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        observed,
        "request body should spill before the client reset"
    );

    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("connection handler should stop after the client reset")
        .unwrap();
    assert!(
        !contains_generated_temp_file(&temp_root),
        "client cancellation must remove the partial spool file"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_invalid_buffered_chunk_before_a_return_response() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18736;
                    location / { return 200 "TEST\n"; }
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\n\
         Connection: close\r\n\r\n4\r\nSEE-THIS\r\n0\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

fn response_header<'a>(response: &'a str, name: &str) -> Option<&'a str> {
    response.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

async fn read_backend_request(stream: &mut tokio::net::TcpStream) -> (String, String) {
    let mut reader = BufReader::new(stream);
    let mut request = String::new();
    let mut content_length = 0;
    let mut line = String::new();
    reader.read_line(&mut request).await.unwrap();
    loop {
        line.clear();
        reader.read_line(&mut line).await.unwrap();
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap();
        }
        request.push_str(&line);
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await.unwrap();
    (
        request,
        String::from_utf8(body).expect("request body is UTF-8"),
    )
}
