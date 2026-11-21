use super::*;

#[test]
fn builds_proxy_requests_with_body_method_and_header_controls() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18099;
                        location /forward/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_set_header X-Set fixed;
                        }
                        location /headers-off/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_pass_request_headers off;
                            proxy_set_header X-Allowed yes;
                        }
                        location /body-off/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_pass_request_body off;
                        }
                        location /custom/ {
                            proxy_pass http://127.0.0.1:19000;
                            proxy_method PATCH;
                            proxy_set_body "custom=$arg_value";
                        }
                    }
                }
            "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let request = Request {
        method: "POST".into(),
        target: "/forward?value=hello".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Host".into(),
                value: "example.test".into(),
            },
            Header {
                name: "Content-Length".into(),
                value: "8".into(),
            },
            Header {
                name: "Expect".into(),
                value: "100-continue".into(),
            },
            Header {
                name: "X-Original".into(),
                value: "original".into(),
            },
        ],
        body_stream: None,
        body: b"original".to_vec(),
        remote_addr: "127.0.0.1:1234".into(),
        local_addr: "127.0.0.1:18099".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:1234".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18099".parse().unwrap();
    let mut variables = Variables::new(&request, "example.test", server, &local, &remote);
    variables.proxy_host = "127.0.0.1:19000".into();
    let env = RequestEnv {
        uri: normalize_uri("/forward").unwrap(),
        args: Some("value=hello".into()),
        unparsed_uri: Some("/forward?value=hello".into()),
        internal: false,
        bypass_try_files: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };

    fn header<'a>(message: &'a ProxyRequestMessage, name: &str) -> Option<&'a str> {
        message
            .headers
            .iter()
            .find(|(header, _)| header.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    let location = &server.locations[0];
    let forwarded = build_proxy_request(
        location.context.proxy_pass.as_ref().unwrap(),
        &request,
        &env,
        Some(location),
        &location.context,
        &variables,
    );
    assert_eq!(forwarded.method, "POST");
    assert_eq!(forwarded.body, b"original");
    assert_eq!(header(&forwarded, "Host"), Some("127.0.0.1:19000"));
    assert_eq!(header(&forwarded, "X-Original"), Some("original"));
    assert_eq!(header(&forwarded, "X-Set"), Some("fixed"));
    assert_eq!(header(&forwarded, "Content-Length"), Some("8"));
    assert_eq!(header(&forwarded, "Expect"), None);
    assert_eq!(header(&forwarded, "Connection"), None);

    let location = &server.locations[1];
    let headers_off = build_proxy_request(
        location.context.proxy_pass.as_ref().unwrap(),
        &request,
        &env,
        Some(location),
        &location.context,
        &variables,
    );
    assert_eq!(headers_off.body, b"original");
    assert_eq!(header(&headers_off, "X-Original"), None);
    assert_eq!(header(&headers_off, "X-Set"), None);
    assert_eq!(header(&headers_off, "X-Allowed"), Some("yes"));

    let location = &server.locations[2];
    let body_off = build_proxy_request(
        location.context.proxy_pass.as_ref().unwrap(),
        &request,
        &env,
        Some(location),
        &location.context,
        &variables,
    );
    assert!(body_off.body.is_empty());
    assert_eq!(header(&body_off, "Content-Length"), None);

    let location = &server.locations[3];
    let custom = build_proxy_request(
        location.context.proxy_pass.as_ref().unwrap(),
        &request,
        &env,
        Some(location),
        &location.context,
        &variables,
    );
    assert_eq!(custom.method, "PATCH");
    assert_eq!(custom.body, b"custom=hello");
    assert_eq!(header(&custom, "Content-Length"), Some("12"));
    let _ = std::fs::remove_dir_all(prefix);
}
