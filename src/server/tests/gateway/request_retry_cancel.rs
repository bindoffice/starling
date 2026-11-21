use super::*;

#[cfg(unix)]
#[tokio::test]
async fn unix_client_close_cancels_fastcgi_retry_and_cleans_body_file() {
    run_gateway_cancel(Gateway::FastCgi).await;
}

#[cfg(unix)]
#[tokio::test]
async fn unix_client_close_cancels_uwsgi_retry_and_cleans_body_file() {
    run_gateway_cancel(Gateway::Uwsgi).await;
}

#[cfg(unix)]
#[tokio::test]
async fn unix_client_close_cancels_scgi_retry_and_cleans_body_file() {
    run_gateway_cancel(Gateway::Scgi).await;
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum Gateway {
    FastCgi,
    Uwsgi,
    Scgi,
}

#[cfg(unix)]
impl Gateway {
    fn directive(self) -> &'static str {
        match self {
            Self::FastCgi => "fastcgi",
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FastCgi => "FastCGI",
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }
}

#[cfg(unix)]
async fn run_gateway_cancel(gateway: Gateway) {
    let prefix_name = gateway.directive();
    let first_path = test_socket_path(&format!("{prefix_name}-retry-cancel-first"));
    let second_path = test_socket_path(&format!("{prefix_name}-retry-cancel-second"));
    let frontend_path = test_socket_path(&format!("{prefix_name}-retry-cancel-frontend"));
    let Some(first) = test_unix_listener(&first_path) else {
        return;
    };
    let Some(second) = test_unix_listener(&second_path) else {
        return;
    };
    let Some(frontend_listener) = test_unix_listener(&frontend_path) else {
        return;
    };
    let (request_seen, request_seen_rx) = tokio::sync::oneshot::channel();
    let (release, release_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        let body = match gateway {
            Gateway::FastCgi => {
                let mut body = Vec::new();
                loop {
                    let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
                    if record_type != FCGI_STDIN {
                        continue;
                    }
                    if content.is_empty() {
                        break;
                    }
                    body.extend_from_slice(&content);
                }
                body
            }
            Gateway::Uwsgi => {
                read_uwsgi_header(&mut stream).await;
                let mut body = [0_u8; 8];
                stream.read_exact(&mut body).await.unwrap();
                body.to_vec()
            }
            Gateway::Scgi => {
                read_scgi_header(&mut stream).await;
                let mut body = [0_u8; 8];
                stream.read_exact(&mut body).await.unwrap();
                body.to_vec()
            }
        };
        assert_eq!(body, b"partmore");
        request_seen.send(()).unwrap();
        release_rx.await.unwrap();
        let response = b"Status: 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
        match gateway {
            Gateway::FastCgi => {
                let _ = write_fastcgi_record(&mut stream, FCGI_STDOUT, response).await;
                let _ = write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8]).await;
                let _ = stream.flush().await;
            }
            Gateway::Uwsgi | Gateway::Scgi => {
                let _ = stream.write_all(response).await;
                let _ = stream.shutdown().await;
            }
        }
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!("gateway retry started after the Unix client closed");
        }
    });

    let directive = gateway.directive();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream gateway_retry {{
                        server unix:{};
                        server unix:{};
                    }}
                    server {{
                        listen unix:{};
                        location / {{
                            {directive}_pass gateway_retry;
                            {directive}_next_upstream http_500 non_idempotent;
                            {directive}_next_upstream_tries 2;
                            {directive}_buffering off;
                        }}
                    }}
                }}
            "#,
            first_path.display(),
            second_path.display(),
            frontend_path.display(),
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let (shutdown, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let server_task = tokio::spawn(accept_unix_loop(
        frontend_listener,
        group,
        state,
        shutdown_rx,
    ));

    let mut client = UnixStream::connect(&frontend_path).await.unwrap();
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npartmore",
        )
        .await
        .unwrap();
    request_seen_rx.await.unwrap();
    drop(client);
    tokio::time::sleep(Duration::from_millis(150)).await;
    release.send(()).unwrap();

    first_backend.await.unwrap();
    second_backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "cancelled {} retry attempt leaked its file-backed request body",
        gateway.label()
    );
    shutdown.send(ShutdownMode::Fast).unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("Unix accept loop should stop")
        .unwrap();

    let _ = std::fs::remove_file(first_path);
    let _ = std::fs::remove_file(second_path);
    let _ = std::fs::remove_file(frontend_path);
    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
async fn wait_for_temp_cleanup(path: &std::path::Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
