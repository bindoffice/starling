use super::*;

#[test]
fn builds_grpc_requests_from_the_original_uri_and_headers() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18570;
                    location / {
                        grpc_pass grpc://backend.test;
                        grpc_set_header Host api.example.test;
                        grpc_set_header X-Configured yes;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Call?trace=1".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Connection".into(),
                value: "keep-alive".into(),
            },
            Header {
                name: "TE".into(),
                value: "trailers".into(),
            },
            Header {
                name: "Content-Type".into(),
                value: "application/grpc".into(),
            },
            Header {
                name: "X-Configured".into(),
                value: "forwarded-value".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
        ])
        .into(),
        body_stream: None,
        body: b"hello".to_vec(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18570".into(),
        keep_alive: true,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18570".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/pkg.Service/Call".into(),
        args: Some("trace=1".into()),
        unparsed_uri: Some("/pkg.Service/Call?trace=1".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };

    let outgoing =
        build_grpc_request(&request, &env, context, &variables, "backend.test", false).unwrap();
    assert_eq!(outgoing.body, b"hello");
    assert_eq!(outgoing.request.method(), http::Method::POST);
    assert_eq!(outgoing.request.uri().scheme_str(), Some("http"));
    assert_eq!(
        outgoing
            .request
            .uri()
            .authority()
            .map(|value| value.as_str()),
        Some("api.example.test")
    );
    assert_eq!(
        outgoing.request.uri().path_and_query().unwrap().as_str(),
        "/pkg.Service/Call?trace=1"
    );
    assert_eq!(
        outgoing
            .request
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/grpc"
    );
    assert_eq!(
        outgoing
            .request
            .headers()
            .get("te")
            .unwrap()
            .to_str()
            .unwrap(),
        "trailers"
    );
    assert_eq!(
        outgoing
            .request
            .headers()
            .get("x-configured")
            .unwrap()
            .to_str()
            .unwrap(),
        "yes"
    );
    assert_eq!(
        outgoing
            .request
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        "5"
    );
    assert!(outgoing.request.headers().get("host").is_none());
    assert!(outgoing.request.headers().get("connection").is_none());

    let mut tls_env = env;
    tls_env.unparsed_uri = Some("/pkg.Service/Tls".into());
    let tls = build_grpc_request(
        &request,
        &tls_env,
        context,
        &variables,
        "secure.example.test",
        true,
    )
    .unwrap();
    assert_eq!(tls.request.uri().scheme_str(), Some("https"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn preserves_streaming_grpc_request_body_and_content_length() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18571;
                    location / {
                        grpc_pass grpc://backend.test;
                        grpc_request_buffering off;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let (_sender, body_stream) = crate::http::RequestBodyStream::channel();
    let request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Call".into(),
        version: "HTTP/2.0".into(),
        headers: (vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Type".into(),
                value: "application/grpc".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
        ])
        .into(),
        body: Vec::new(),
        body_stream: Some(body_stream),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18571".into(),
        keep_alive: true,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18571".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/pkg.Service/Call".into(),
        args: None,
        unparsed_uri: Some("/pkg.Service/Call".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };

    let outgoing =
        build_grpc_request(&request, &env, context, &variables, "backend.test", false).unwrap();
    assert!(outgoing.body.is_empty());
    assert!(outgoing.body_stream.is_some());
    assert_eq!(
        outgoing
            .request
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap(),
        "5"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn default_grpc_request_buffering_does_not_stream() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18572;
                    location / {
                        grpc_pass grpc://backend.test;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let (_sender, body_stream) = crate::http::RequestBodyStream::channel();
    let request = Request {
        method: "POST".into(),
        target: "/pkg.Service/Call".into(),
        version: "HTTP/2.0".into(),
        headers: (vec![
            Header {
                name: "Host".into(),
                value: "localhost".into(),
            },
            Header {
                name: "Content-Type".into(),
                value: "application/grpc".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "5".into(),
            },
        ])
        .into(),
        body: b"hello".to_vec(),
        body_stream: Some(body_stream),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18572".into(),
        keep_alive: true,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18572".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let env = RequestEnv {
        uri: "/pkg.Service/Call".into(),
        args: None,
        unparsed_uri: Some("/pkg.Service/Call".into()),
        valid_location: true,
        internal: false,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };

    let outgoing =
        build_grpc_request(&request, &env, context, &variables, "backend.test", false).unwrap();
    assert!(outgoing.body_stream.is_none());
    assert_eq!(outgoing.body, b"hello");
    let _ = std::fs::remove_dir_all(prefix);
}
