use super::*;

#[test]
fn preserves_unix_paths_in_named_gateway_upstreams() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    upstream php {
                        server unix:/run/php.sock;
                    }
                    server {
                        listen 127.0.0.1:18129;
                        location / { fastcgi_pass php; }
                    }
                }
            "#,
        &[],
    );
    assert_eq!(
        parse_upstream_address("unix:/run/php.sock"),
        Some(("unix:/run/php.sock".into(), 0))
    );
    let upstream = config.http.as_ref().unwrap().upstreams.get("php").unwrap();
    let targets = resolve_upstream_config(upstream, None, None);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].host, "unix:/run/php.sock");
    assert_eq!(targets[0].port, 0);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn proxies_fastcgi_over_a_unix_socket() {
    let socket_path = test_socket_path("fastcgi");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut parameters = Vec::new();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            match record_type {
                FCGI_PARAMS if !content.is_empty() => parameters.extend(content),
                FCGI_STDIN if content.is_empty() => break,
                _ => {}
            }
        }
        let values = parse_fastcgi_test_params(&parameters);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("GET"),
            "{values:?}"
        );
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 201 Created\r\nContent-Type: text/plain\r\n\r\nunix-fastcgi",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18130;
                        location / {{
                            fastcgi_pass unix:{};
                            fastcgi_param REQUEST_METHOD $request_method;
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 201 Created\r\n"),
        "{response}"
    );
    assert!(response.ends_with("unix-fastcgi"), "{response}");
    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn proxies_uwsgi_through_a_named_unix_upstream() {
    let socket_path = test_socket_path("uwsgi");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let parameter_length = u16::from_le_bytes([header[1], header[2]]) as usize;
        let mut parameters = vec![0_u8; parameter_length];
        stream.read_exact(&mut parameters).await.unwrap();
        let values = parse_null_separated_params(&parameters);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("POST")
        );
        let mut body = [0_u8; 4];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"test");
        stream
            .write_all(b"Status: 202 Accepted\r\nContent-Type: text/plain\r\n\r\nunix-uwsgi")
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream python {{
                        server unix:{};
                    }}
                    server {{
                        listen 127.0.0.1:18131;
                        location / {{
                            uwsgi_pass python;
                            uwsgi_param REQUEST_METHOD $request_method;
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
            &config,
            "POST /run HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\ntest",
        )
        .await;
    assert!(
        response.starts_with("HTTP/1.1 202 Accepted\r\n"),
        "{response}"
    );
    assert!(response.ends_with("unix-uwsgi"), "{response}");
    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn proxies_scgi_over_a_unix_socket() {
    let socket_path = test_socket_path("scgi");
    let Some(listener) = test_unix_listener(&socket_path) else {
        return;
    };
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut prefix = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            if byte[0] == b':' {
                break;
            }
            prefix.push(byte[0]);
        }
        let length = std::str::from_utf8(&prefix)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut parameters = vec![0_u8; length];
        stream.read_exact(&mut parameters).await.unwrap();
        let mut comma = [0_u8; 1];
        stream.read_exact(&mut comma).await.unwrap();
        assert_eq!(comma, [b',']);
        let values = parse_null_separated_params(&parameters);
        assert_eq!(
            values.get("REQUEST_METHOD").map(String::as_str),
            Some("GET")
        );
        stream
                .write_all(b"Status: 203 Non-Authoritative Information\r\nContent-Type: text/plain\r\n\r\nunix-scgi")
                .await
                .unwrap();
        stream.shutdown().await.unwrap();
    });
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18132;
                        location / {{
                            scgi_pass unix:{};
                            scgi_param REQUEST_METHOD $request_method;
                        }}
                    }}
                }}
            "#,
        socket_path.display()
    );
    let (prefix, config) = test_config(&source, &[]);
    let response = request_body(
        &config,
        "GET /scgi HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 203 Unknown Status\r\n"),
        "{response}"
    );
    assert!(response.ends_with("unix-scgi"), "{response}");
    backend.await.unwrap();
    let _ = std::fs::remove_file(socket_path);
    let _ = std::fs::remove_dir_all(prefix);
}
