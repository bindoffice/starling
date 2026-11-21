use super::*;

#[cfg(unix)]
#[tokio::test]
async fn named_upstream_retries_file_backed_http2_request_body_from_the_start() {
    let first_path = test_socket_path("h2-retry-first");
    let second_path = test_socket_path("h2-retry-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_path_display = first_path.display();
    let second_path_display = second_path.display();
    let first_backend = serve_h2_request(first, 500, b"first");
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream h2_retry {{
                        server unix:{first_path_display};
                        server unix:{second_path_display};
                    }}
                    server {{
                        listen 127.0.0.1:18590;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 file-backed retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed HTTP/2 request body should be removed after retry"
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn named_upstream_retries_file_backed_http2_request_body_after_connect_failure() {
    let first_path = test_socket_path("h2-retry-connect-missing");
    let second_path = test_socket_path("h2-retry-connect-second");
    let _ = std::fs::remove_file(&first_path);
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_path_display = first_path.display();
    let second_path_display = second_path.display();
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream h2_retry {{
                        server unix:{first_path_display};
                        server unix:{second_path_display};
                    }}
                    server {{
                        listen 127.0.0.1:18662;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream error non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 connect retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed HTTP/2 request body should be removed after connect retry"
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn named_upstream_retries_file_backed_http2_request_body_after_read_timeout() {
    let first_path = test_socket_path("h2-retry-timeout-first");
    let second_path = test_socket_path("h2-retry-timeout-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_path_display = first_path.display();
    let second_path_display = second_path.display();
    let first_backend =
        serve_h2_request_with_reply(first, H2TestReply::Delayed(Duration::from_millis(200)));
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream h2_retry {{
                        server unix:{first_path_display};
                        server unix:{second_path_display};
                    }}
                    server {{
                        listen 127.0.0.1:18610;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_read_timeout 20ms;
                            proxy_next_upstream timeout non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 timeout retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed HTTP/2 request body should be removed after timeout retry"
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn named_upstream_retries_file_backed_http2_request_body_after_invalid_response() {
    let first_path = test_socket_path("h2-retry-invalid-first");
    let second_path = test_socket_path("h2-retry-invalid-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_path_display = first_path.display();
    let second_path_display = second_path.display();
    let first_backend = serve_h2_request_with_reply(first, H2TestReply::NoResponse);
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream h2_retry {{
                        server unix:{first_path_display};
                        server unix:{second_path_display};
                    }}
                    server {{
                        listen 127.0.0.1:18611;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream invalid_header non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 invalid-response retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed HTTP/2 request body should be removed after invalid-response retry"
    );
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn failed_http2_attempt_informational_response_does_not_leak_into_retry() {
    let first_path = test_socket_path("h2-retry-informational-first");
    let second_path = test_socket_path("h2-retry-informational-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_backend = serve_h2_request_with_reply(
        first,
        H2TestReply::InformationalThenFinal {
            status: 500,
            body: b"first",
        },
    );
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    early_hints 1;
                    upstream h2_retry {{
                        server unix:{};
                        server unix:{};
                    }}
                    server {{
                        listen 127.0.0.1:18722;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream http_500 non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
            first_path.display(),
            second_path.display(),
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 informational retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert!(!response.contains("103 Early Hints"), "{response}");
    assert!(!response.contains("discarded.css"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn http2_informational_response_followed_by_truncation_is_discarded_before_retry() {
    let first_path = test_socket_path("h2-retry-informational-truncated-first");
    let second_path = test_socket_path("h2-retry-informational-truncated-second");
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let first_backend = serve_h2_request_with_reply(first, H2TestReply::InformationalThenDrop);
    let second_backend = serve_h2_request(second, 200, b"ok");

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    early_hints 1;
                    upstream h2_retry {{
                        server unix:{};
                        server unix:{};
                    }}
                    server {{
                        listen 127.0.0.1:18723;
                        location / {{
                            proxy_pass http://h2_retry/;
                            proxy_http_version 2;
                            proxy_next_upstream invalid_header non_idempotent;
                            proxy_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#,
            first_path.display(),
            second_path.display(),
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST /retry HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("HTTP/2 truncated informational retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert!(!response.contains("103 Early Hints"), "{response}");
    assert!(!response.contains("discarded.css"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
fn serve_h2_request(
    listener: tokio::net::UnixListener,
    status: u16,
    response_body: &'static [u8],
) -> tokio::task::JoinHandle<Vec<u8>> {
    serve_h2_request_with_reply(
        listener,
        H2TestReply::Response {
            status,
            body: response_body,
        },
    )
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum H2TestReply {
    Response { status: u16, body: &'static [u8] },
    InformationalThenFinal { status: u16, body: &'static [u8] },
    InformationalThenDrop,
    Delayed(Duration),
    NoResponse,
}

#[cfg(unix)]
fn serve_h2_request_with_reply(
    listener: tokio::net::UnixListener,
    reply: H2TestReply,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            let mut body = Vec::new();
            let mut request_body = request.into_body();
            while let Some(chunk) = request_body.data().await {
                body.extend_from_slice(&chunk.unwrap());
            }
            match reply {
                H2TestReply::Response { status, body } => {
                    let response = http::Response::builder().status(status).body(()).unwrap();
                    let mut stream = respond.send_response(response, false).unwrap();
                    stream.send_data(Bytes::from_static(body), true).unwrap();
                }
                H2TestReply::InformationalThenFinal { status, body } => {
                    send_informational(&mut respond);
                    let response = http::Response::builder().status(status).body(()).unwrap();
                    let mut stream = respond.send_response(response, false).unwrap();
                    stream.send_data(Bytes::from_static(body), true).unwrap();
                }
                H2TestReply::InformationalThenDrop => send_informational(&mut respond),
                H2TestReply::Delayed(delay) => {
                    tokio::time::sleep(delay).await;
                    drop(respond);
                }
                H2TestReply::NoResponse => drop(respond),
            }
            body
        };
        tokio::pin!(work);
        let body = loop {
            tokio::select! {
                body = &mut work => break body,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        break work.await;
                    }
                }
            }
        };
        shutdown_h2(connection).await;
        body
    })
}

#[cfg(unix)]
fn send_informational(respond: &mut h2::server::SendResponse<Bytes>) {
    let response = http::Response::builder()
        .status(http::StatusCode::EARLY_HINTS)
        .header("Link", "</discarded.css>; rel=preload")
        .body(())
        .unwrap();
    respond.send_informational(response).unwrap();
}
