use super::*;

#[tokio::test]
async fn nested_try_files_fallback_rematches_from_root_and_replaces_args() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18365;
                        location /outer/ {
                            try_files /missing /wrong;
                            location /outer/deep/ {
                                try_files /missing /final?source=nested;
                            }
                        }
                        location = /wrong {
                            return 200 "wrong\n";
                        }
                        location = /final {
                            add_header X-Final yes always;
                            return 200 "$uri|$args";
                        }
                    }
                }
            "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET /outer/deep/item?original=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("X-Final: yes\r\n"), "{response}");
    assert!(response.ends_with("/final|source=nested"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_try_files_fallback_uri_replaces_or_clears_args() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18366;
                        location /with-args {
                            try_files /missing @with_args;
                        }
                        location @with_args {
                            try_files /missing /target?from=named;
                        }
                        location /without-args {
                            try_files /missing @without_args;
                        }
                        location @without_args {
                            try_files /missing /target;
                        }
                        location = /target {
                            return 200 "$uri|$args";
                        }
                    }
                }
            "#,
        &[],
    );

    let replaced = request_body(
        &config,
        "GET /with-args?original=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(replaced.ends_with("/target|from=named"), "{replaced}");

    let cleared = request_body(
        &config,
        "GET /without-args?original=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(cleared.ends_with("/target|"), "{cleared}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_try_files_fallback_status_can_use_error_page() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18367;
                        error_page 418 /status-page;
                        location /status-entry {
                            try_files /missing @status;
                        }
                        location @status {
                            try_files /missing =418;
                        }
                        location = /status-page {
                            add_header X-Status-Page yes always;
                            return 200 "status-page:$uri";
                        }
                    }
                }
            "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET /status-entry HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 418 "),
        "unexpected status:\n{response}"
    );
    assert!(response.contains("X-Status-Page: yes\r\n"), "{response}");
    assert!(response.ends_with("status-page:/status-page"), "{response}");
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
        local_addr: "127.0.0.1:18368".into(),
        keep_alive: false,
    }
}

fn initial_env(uri: &str) -> RequestEnv {
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

#[tokio::test]
async fn post_action_chain_uses_the_error_page_location() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18368;
                        error_page 404 /recover;
                        location = /after {
                            try_files /missing =404;
                        }
                        location = /recover {
                            post_action /final-after;
                            return 200 "recover";
                        }
                        location = /final-after {
                            return 203 "final-after";
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = request("/after");
    let local = ListenAddress::Inet("127.0.0.1:18368".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:45678".parse().unwrap());
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);

    let processed = execute_post_actions(
        http,
        server,
        &request,
        initial_env("/after"),
        &mut variables,
        "/after".into(),
        1,
    )
    .await
    .expect("error-page post_action chain");

    assert_eq!(processed.response.status, 203);
    let body = processed
        .response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .expect("post_action body");
    assert_eq!(body, b"final-after");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn error_page_does_not_leak_the_triggering_location_post_action() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18369;
                        error_page 404 /recover;
                        location = /main {
                            post_action /main-after;
                            return 404;
                        }
                        location = /recover {
                            return 200 "recover";
                        }
                        location = /main-after {
                            return 201 "leak";
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = request("/main");
    let local = ListenAddress::Inet("127.0.0.1:18369".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:45678".parse().unwrap());
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);

    let processed = process_request(
        http,
        server,
        &mut request.clone(),
        initial_env("/main"),
        &mut variables,
        1,
    )
    .await;

    assert_eq!(processed.response.status, 404);
    assert_eq!(processed.context.post_action, None);
    let _ = std::fs::remove_dir_all(prefix);
}
