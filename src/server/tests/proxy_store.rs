use super::support::*;
use super::*;

#[tokio::test]
async fn stores_complete_proxy_response_for_proxy_store_on() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18130;
                    location /files {
                        proxy_store on;
                        proxy_store_access user:r group:r;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let request = request("/files/report.txt");
    let variables = request_variables(server, &request);
    let env = request_env("/files/report.txt");
    let response = upstream_response(200, b"stored body", Some(11), &[]);
    let target_dir = prefix.join("html/files");
    std::fs::create_dir_all(&target_dir).unwrap();
    let stale =
        dead_process_id().map(|owner| target_dir.join(format!(".starling-store-{owner}-1")));
    let active = target_dir.join(format!(".starling-store-{}-2", std::process::id()));
    let sentinel = target_dir.join("sentinel");
    std::fs::write(&active, b"active").unwrap();
    std::fs::write(&sentinel, b"keep").unwrap();
    if let Some(stale) = &stale {
        std::fs::write(stale, b"stale").unwrap();
    }

    let stored = store_proxy_response(
        &response,
        &env,
        Some(location),
        &location.context,
        &variables,
    )
    .await
    .unwrap();
    let expected = prefix.join("html/files/report.txt");
    assert_eq!(stored.as_deref(), Some(expected.as_path()));
    assert_eq!(std::fs::read(&expected).unwrap(), b"stored body");
    assert!(active.exists());
    assert!(sentinel.exists());
    if let Some(stale) = stale {
        assert!(!stale.exists());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&expected).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o440);
    }
    let _ = std::fs::remove_dir_all(prefix);
}

fn dead_process_id() -> Option<u32> {
    (2..100_000).find(|pid| {
        *pid != std::process::id() && !crate::server::temp_cleanup::process_is_alive(*pid)
    })
}

#[tokio::test]
async fn stores_file_backed_proxy_responses() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18139;
                    location /files {
                        proxy_store on;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let request = request("/files/report.txt");
    let variables = request_variables(server, &request);
    let body_path = prefix.join("upstream-body");
    std::fs::write(&body_path, b"file-backed body").unwrap();
    let file = tokio::fs::File::open(&body_path).await.unwrap();
    let response = UpstreamResponse {
        status: 200,
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Content-Length".into(),
            value: "16".into(),
        }],
        trailers: Vec::new(),
        early_hints: Vec::new(),
        body: ResponseBody::File {
            file,
            offset: 0,
            length: 16,
        },
        body_temp_file: None,
    };

    let stored = store_proxy_response(
        &response,
        &request_env("/files/report.txt"),
        Some(location),
        &location.context,
        &variables,
    )
    .await
    .unwrap();
    let expected = prefix.join("html/files/report.txt");
    assert_eq!(stored.as_deref(), Some(expected.as_path()));
    assert_eq!(std::fs::read(&expected).unwrap(), b"file-backed body");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn expands_store_path_variables_and_sets_last_modified() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18131;
                    location /custom {
                        proxy_store store/$host/$upstream_status$uri;
                        proxy_store_access user:r;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let request = request("/custom/report.txt");
    let variables = request_variables(server, &request);
    let env = request_env("/custom/report.txt");
    let last_modified = "Wed, 21 Oct 2015 07:28:00 GMT";
    let response = upstream_response(
        200,
        b"variable path",
        Some(13),
        &[("Last-Modified", last_modified)],
    );

    let stored = store_proxy_response(
        &response,
        &env,
        Some(location),
        &location.context,
        &variables,
    )
    .await
    .unwrap();
    let expected = prefix.join("store/localhost/200/custom/report.txt");
    assert_eq!(stored.as_deref(), Some(expected.as_path()));
    assert_eq!(std::fs::read(&expected).unwrap(), b"variable path");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&expected).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o400);
    }

    let expected_modified = httpdate::parse_http_date(last_modified).unwrap();
    let modified = std::fs::metadata(&expected).unwrap().modified().unwrap();
    assert_eq!(
        modified.duration_since(UNIX_EPOCH).unwrap().as_secs(),
        expected_modified
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn skips_store_for_incomplete_or_non_200_responses() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18132;
                    location /off {}
                    location /incomplete {
                        proxy_store on;
                    }
                    location /status {
                        proxy_store on;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];

    let off_request = request("/off/file.txt");
    let off_variables = request_variables(server, &off_request);
    let off_response = upstream_response(200, b"body", Some(4), &[]);
    assert!(
        store_proxy_response(
            &off_response,
            &request_env("/off/file.txt"),
            Some(&server.locations[0]),
            &server.locations[0].context,
            &off_variables,
        )
        .await
        .unwrap()
        .is_none()
    );

    let incomplete_request = request("/incomplete/file.txt");
    let incomplete_variables = request_variables(server, &incomplete_request);
    let incomplete_response = upstream_response(200, b"short", Some(10), &[]);
    assert!(
        store_proxy_response(
            &incomplete_response,
            &request_env("/incomplete/file.txt"),
            Some(&server.locations[1]),
            &server.locations[1].context,
            &incomplete_variables,
        )
        .await
        .unwrap()
        .is_none()
    );

    let status_request = request("/status/file.txt");
    let status_variables = request_variables(server, &status_request);
    let status_response = upstream_response(206, b"partial", Some(7), &[]);
    assert!(
        store_proxy_response(
            &status_response,
            &request_env("/status/file.txt"),
            Some(&server.locations[2]),
            &server.locations[2].context,
            &status_variables,
        )
        .await
        .unwrap()
        .is_none()
    );
    assert!(!prefix.join("html/incomplete/file.txt").exists());
    assert!(!prefix.join("html/status/file.txt").exists());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reports_store_write_failures_without_creating_the_target() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18133;
                    location /failure {
                        proxy_store blocker/child;
                    }
                }
            }
        "#,
        &[],
    );
    std::fs::write(prefix.join("blocker"), b"not a directory").unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    let location = &server.locations[0];
    let request = request("/failure/file.txt");
    let variables = request_variables(server, &request);
    let response = upstream_response(200, b"body", Some(4), &[]);

    let result = store_proxy_response(
        &response,
        &request_env("/failure/file.txt"),
        Some(location),
        &location.context,
        &variables,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(prefix.join("blocker")).unwrap(),
        b"not a directory"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stores_proxy_responses_end_to_end() {
    let upstream = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping proxy_store TCP test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nproxied\n",
            )
            .await
            .unwrap();
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18134;
                        location /stored {{
                            proxy_pass http://{upstream_address};
                            proxy_store on;
                            proxy_store_access user:r group:r;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let response = request_body(
        &config,
        "GET /stored/file.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("proxied\n"), "{response}");
    assert_eq!(
        std::fs::read(prefix.join("html/stored/file.txt")).unwrap(),
        b"proxied\n"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

fn request(target: &str) -> Request {
    Request {
        method: "GET".into(),
        target: target.into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18130".into(),
        keep_alive: false,
    }
}

fn request_env(uri: &str) -> RequestEnv {
    RequestEnv {
        uri: uri.into(),
        args: None,
        unparsed_uri: Some(uri.into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    }
}

fn request_variables(server: &ServerConfig, request: &Request) -> Variables {
    let remote: SocketAddr = request.remote_addr.parse().unwrap();
    let local: SocketAddr = request.local_addr.parse().unwrap();
    Variables::new(request, "localhost", server, &local, &remote)
}

fn upstream_response(
    status: u16,
    body: &[u8],
    content_length: Option<u64>,
    headers: &[(&str, &str)],
) -> UpstreamResponse {
    let mut response_headers = headers
        .iter()
        .map(|(name, value)| Header {
            name: (*name).into(),
            value: (*value).into(),
        })
        .collect::<Vec<_>>();
    if let Some(length) = content_length {
        response_headers.push(Header {
            name: "Content-Length".into(),
            value: length.to_string(),
        });
    }
    UpstreamResponse {
        status,
        version: "HTTP/1.1".into(),
        headers: response_headers,
        trailers: Vec::new(),
        early_hints: Vec::new(),
        body: ResponseBody::Bytes(body.to_vec()),
        body_temp_file: None,
    }
}
