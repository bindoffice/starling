use super::support::*;
use super::*;

#[tokio::test]
async fn retries_grpc_upstreams_on_configured_status() {
    let Some(first) = bind_grpc_listener("gRPC retry first upstream").await else {
        return;
    };
    let Some(second) = bind_grpc_listener("gRPC retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(serve_grpc_response(first, 500, b"first", "13"));
    let second_backend = tokio::spawn(serve_grpc_response(second, 200, b"second", "0"));

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream grpc_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18574;
                        location / {{
                            grpc_pass grpc_retry;
                            grpc_next_upstream http_500 non_idempotent;
                            grpc_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Retry".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Content-Type".into(),
            value: "application/grpc".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18574".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18574".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/pkg.Service/Retry".into(),
        args: None,
        unparsed_uri: Some("/pkg.Service/Retry".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let pass = context.grpc.pass.as_ref().unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        grpc_request(pass, http, &request, &env, context, &mut variables),
    )
    .await
    .expect("gRPC retry exchange");

    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"second"
    );
    assert_eq!(variables.upstream_status, "500, 200");
    assert!(variables.upstream_addr.contains(&first_address.to_string()));
    assert!(
        variables
            .upstream_addr
            .contains(&second_address.to_string())
    );
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn retries_file_backed_grpc_request_body_from_the_start() {
    let Some(first) = bind_grpc_listener("gRPC request retry first upstream").await else {
        return;
    };
    let Some(second) = bind_grpc_listener("gRPC request retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(serve_grpc_response_with_body(first, 500, b"first", "13"));
    let second_backend = tokio::spawn(serve_grpc_response_with_body(second, 200, b"second", "0"));

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream grpc_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18589;
                        location / {{
                            grpc_pass grpc_retry;
                            grpc_next_upstream http_500 non_idempotent;
                            grpc_next_upstream_tries 2;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST /pkg.Service/Retry HTTP/1.1\r\nHost: localhost\r\n\
         Content-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("second"), "{response}");
    assert!(response.contains("grpc-status: 0\r\n"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed gRPC request body should be removed after retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn retries_file_backed_grpc_request_body_after_connect_failure() {
    let Some(unavailable) = bind_grpc_listener("gRPC connect retry unavailable upstream").await
    else {
        return;
    };
    let Some(second) = bind_grpc_listener("gRPC connect retry second upstream").await else {
        return;
    };
    let unavailable_address = unavailable.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    drop(unavailable);
    let second_backend = tokio::spawn(serve_grpc_response_with_body(second, 200, b"second", "0"));

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream grpc_retry {{
                        server {unavailable_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18663;
                        location / {{
                            grpc_pass grpc_retry;
                            grpc_next_upstream error non_idempotent;
                            grpc_next_upstream_tries 2;
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
            "POST /pkg.Service/Retry HTTP/1.1\r\nHost: localhost\r\n\
             Content-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("gRPC connect retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("second"), "{response}");
    assert!(response.contains("grpc-status: 0\r\n"), "{response}");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed gRPC request body should be removed after connect retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn retries_file_backed_grpc_request_body_after_read_timeout() {
    let Some(first) = bind_grpc_listener("gRPC timeout retry first upstream").await else {
        return;
    };
    let Some(second) = bind_grpc_listener("gRPC timeout retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(serve_grpc_reply(
        first,
        GrpcTestReply::Delayed(Duration::from_millis(200)),
    ));
    let second_backend = tokio::spawn(serve_grpc_response_with_body(second, 200, b"second", "0"));

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream grpc_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18640;
                        location / {{
                            grpc_pass grpc_retry;
                            grpc_read_timeout 20ms;
                            grpc_next_upstream timeout non_idempotent;
                            grpc_next_upstream_tries 2;
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
            "POST /pkg.Service/Retry HTTP/1.1\r\nHost: localhost\r\n\
             Content-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("gRPC timeout retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("second"), "{response}");
    assert!(response.contains("grpc-status: 0\r\n"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed gRPC request body should be removed after timeout retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn retries_file_backed_grpc_request_body_after_invalid_response() {
    let Some(first) = bind_grpc_listener("gRPC invalid retry first upstream").await else {
        return;
    };
    let Some(second) = bind_grpc_listener("gRPC invalid retry second upstream").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = tokio::spawn(serve_grpc_reply(first, GrpcTestReply::NoResponse));
    let second_backend = tokio::spawn(serve_grpc_response_with_body(second, 200, b"second", "0"));

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream grpc_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18641;
                        location / {{
                            grpc_pass grpc_retry;
                            grpc_next_upstream invalid_header non_idempotent;
                            grpc_next_upstream_tries 2;
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
            "POST /pkg.Service/Retry HTTP/1.1\r\nHost: localhost\r\n\
             Content-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .expect("gRPC invalid-response retry should finish");
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("second"), "{response}");
    assert!(response.contains("grpc-status: 0\r\n"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed gRPC request body should be removed after invalid-response retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
