use super::*;

#[tokio::test]
async fn named_location_runs_try_files_and_refreshes_uri_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18363;
                        root html;
                        location /entry {
                            try_files /missing @named;
                        }
                        location @named {
                            try_files /actual.txt =404;
                            add_header X-URI $uri always;
                            add_header X-Filename $request_filename always;
                        }
                        location /nested {
                            try_files /missing @first;
                        }
                        location @first {
                            try_files /missing @second;
                        }
                        location @second {
                            return 200 "second:$uri";
                        }
                    }
                }
            "#,
        &[("html/actual.txt", "named-file\n")],
    );

    let named = request_body(
        &config,
        "GET /entry HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(named.starts_with("HTTP/1.1 200 OK\r\n"), "{named}");
    assert!(named.ends_with("named-file\n"), "{named}");
    assert!(named.contains("X-URI: /actual.txt\r\n"), "{named}");
    assert!(
        named.contains(&format!(
            "X-Filename: {}\r\n",
            prefix.join("html/actual.txt").display()
        )),
        "{named}"
    );

    let chained = request_body(
        &config,
        "GET /nested HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(chained.starts_with("HTTP/1.1 200 OK\r\n"), "{chained}");
    assert!(chained.ends_with("second:/nested"), "{chained}");
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
        local_addr: "127.0.0.1:18364".into(),
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
async fn post_action_path_can_fall_back_to_a_named_try_files_location() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18364;
                        location = /main {
                            post_action /after;
                            return 200 "main";
                        }
                        location = /after {
                            try_files /missing @after_named;
                        }
                        location @after_named {
                            return 203 "post:$uri";
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = request("/main");
    let local = ListenAddress::Inet("127.0.0.1:18364".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:45678".parse().unwrap());
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);

    let processed = execute_post_actions(
        http,
        server,
        &request,
        initial_env("/main"),
        &mut variables,
        "/after".into(),
        1,
    )
    .await
    .expect("post_action fallback");
    assert_eq!(processed.response.status, 203);
    let body = processed
        .response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .expect("post_action body");
    assert_eq!(body, b"post:/after");
    let _ = std::fs::remove_dir_all(prefix);
}
