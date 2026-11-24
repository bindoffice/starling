use super::*;

#[tokio::test]
async fn applies_satisfy_any_and_all_access_semantics() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    auth_basic "Restricted";
                    auth_basic_user_file conf/htpasswd;
                    server {
                        listen 127.0.0.1:18118;
                        location /any/ {
                            satisfy any;
                            deny all;
                            return 200 "any\n";
                        }
                        location /all/ {
                            satisfy all;
                            deny all;
                            return 200 "all\n";
                        }
                    }
                }
            "#,
        &[("conf/htpasswd", "user:{PLAIN}pass\n")],
    );
    let no_credentials = request_body(
        &config,
        "GET /any/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(no_credentials.starts_with("HTTP/1.1 401 Unauthorized\r\n"));
    let any = request_body(
            &config,
            "GET /any/ HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic dXNlcjpwYXNz\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        any.starts_with("HTTP/1.1 200 OK\r\n") && any.ends_with("any\n"),
        "unexpected satisfy any response:\n{any}"
    );
    let all = request_body(
            &config,
            "GET /all/ HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic dXNlcjpwYXNz\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        all.starts_with("HTTP/1.1 403 Forbidden\r\n"),
        "unexpected satisfy all response:\n{all}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn authorizes_requests_with_auth_request_subrequests() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18117;
                        location = /auth {
                            internal;
                            if ($arg_allow = yes) { return 204; }
                            return 401;
                        }
                        location /private/ {
                            auth_request /auth?allow=$arg_allow;
                            auth_request_set $auth_status $status;
                            return 200 "$auth_status\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let allowed = request_body(
        &config,
        "GET /private/?allow=yes HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        allowed.starts_with("HTTP/1.1 200 OK\r\n") && allowed.ends_with("204\n"),
        "unexpected auth_request allow response:\n{allowed}"
    );
    let denied = request_body(
        &config,
        "GET /private/?allow=no HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        denied.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "unexpected auth_request deny response:\n{denied}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_requests_over_the_limit_req_rate() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    limit_req_zone $binary_remote_addr zone=api:10m rate=1r/s;
                    limit_req zone=api;
                    limit_req_status 429;
                    error_page 429 /limited;
                    server {
                        listen 127.0.0.1:18112;
                        location = /limited {
                            internal;
                            return 200 "$limit_req_status\n";
                        }
                        location / { return 200 "$limit_req_status\n"; }
                    }
                }
            "#,
        &[],
    );
    let state = test_state(&config);
    let first = request_body_with_state(
        &config,
        state.clone(),
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        first.starts_with("HTTP/1.1 200 OK\r\n") && first.ends_with("PASSED\n"),
        "unexpected first limited response:\n{first}"
    );
    let second = request_body_with_state(
        &config,
        state,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        second.starts_with("HTTP/1.1 429 Too Many Requests\r\n") && second.ends_with("REJECTED\n"),
        "unexpected rejected response:\n{second}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn tracks_connection_limit_guards() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    limit_conn_zone $binary_remote_addr zone=perip:10m;
                    limit_conn perip 1;
                    server {
                        listen 127.0.0.1:18113;
                        location / { return 204; }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18113".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18113".parse().unwrap();
    let limits = Arc::new(Mutex::new(LimitState::default()));
    let mut first = Variables::new(&request, "localhost", server, &local, &remote);
    first.limits = limits.clone();
    let guard = acquire_connection_limits(context, &mut first).expect("first connection");
    assert_eq!(first.limit_conn_status, "PASSED");

    let mut second = Variables::new(&request, "localhost", server, &local, &remote);
    second.limits = limits.clone();
    assert_eq!(
        acquire_connection_limits(context, &mut second).err(),
        Some(503)
    );
    assert_eq!(second.limit_conn_status, "REJECTED");

    drop(guard);
    let mut third = Variables::new(&request, "localhost", server, &local, &remote);
    third.limits = limits;
    assert!(acquire_connection_limits(context, &mut third).is_ok());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn keeps_connection_limit_guard_alive_after_request_processing() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    limit_conn_zone $binary_remote_addr zone=perip:10m;
                    limit_conn perip 1;
                    server {
                        listen 127.0.0.1:18119;
                        location / { return 204; }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().expect("http config");
    let server = &http.servers[0];
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18119".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18119".parse().unwrap();
    let limits = Arc::new(Mutex::new(LimitState::default()));
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    variables.limits = limits.clone();
    let env = RequestEnv {
        uri: "/".into(),
        args: None,
        unparsed_uri: Some("/".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    let mut connection_guard = None;
    let mut request = request;
    let _processed = process_request_with_connection_guard(
        http,
        server,
        &mut request,
        env,
        &mut variables,
        0,
        &mut connection_guard,
    )
    .await;
    assert!(connection_guard.is_some());

    let mut second = Variables::new(&request, "localhost", server, &local, &remote);
    second.limits = limits.clone();
    assert_eq!(
        acquire_connection_limits(&server.locations[0].context, &mut second).err(),
        Some(503)
    );

    drop(connection_guard);
    let mut third = Variables::new(&request, "localhost", server, &local, &remote);
    third.limits = limits;
    assert!(acquire_connection_limits(&server.locations[0].context, &mut third).is_ok());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_stub_status_connection_counters() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18115;
                        location = /status { stub_status; }
                    }
                }
            "#,
        &[],
    );
    let status = request_body(
        &config,
        "GET /status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        status.starts_with("HTTP/1.1 200 OK\r\n"),
        "unexpected stub status response:\n{status}"
    );
    assert!(
            status.ends_with(
                "Active connections: 1 \nserver accepts handled requests\n 1 1 1 \nReading: 0 Writing: 1 Waiting: 0 \n"
            ),
            "unexpected stub status body:\n{status}"
        );

    let rejected = request_body(
            &config,
            "POST /status HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(rejected.starts_with("HTTP/1.1 405 Not Allowed\r\n"));
    let _ = std::fs::remove_dir_all(prefix);
}
