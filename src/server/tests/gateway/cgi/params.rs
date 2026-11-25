use super::*;

#[test]
fn merges_forwarded_cgi_headers_without_overriding_explicit_params() {
    let headers = vec![
        Header {
            name: "X-Custom".into(),
            value: "first".into(),
        },
        Header {
            name: "X-Custom".into(),
            value: "second".into(),
        },
        Header {
            name: "Cookie".into(),
            value: "a=1".into(),
        },
        Header {
            name: "Cookie".into(),
            value: "b=2".into(),
        },
    ];
    let mut params = Vec::new();
    append_cgi_request_headers(&mut params, &headers);
    assert_eq!(
        params,
        vec![
            ("HTTP_X_CUSTOM".into(), "first, second".into()),
            ("HTTP_COOKIE".into(), "a=1; b=2".into())
        ]
    );

    let mut params = vec![("HTTP_X_CUSTOM".into(), "configured".into())];
    append_cgi_request_headers(&mut params, &headers);
    assert_eq!(
        params,
        vec![
            ("HTTP_X_CUSTOM".into(), "configured".into()),
            ("HTTP_COOKIE".into(), "a=1; b=2".into())
        ]
    );
}

#[test]
fn cacheable_fastcgi_requests_clear_client_conditional_headers() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                fastcgi_cache_path cache keys_zone=gateway:1m;
                server {
                    listen 127.0.0.1:18148;
                    fastcgi_cache gateway;
                    location / {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_param HTTP_IF_MODIFIED_SINCE configured;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let mut request = plain_request("/", "127.0.0.1:18148");
    for (name, value) in [
        ("If-Modified-Since", "client-ims"),
        ("If-Unmodified-Since", "client-iums"),
        ("If-None-Match", "\"client\""),
        ("If-Match", "\"client\""),
        ("Range", "bytes=0-1"),
        ("If-Range", "\"client\""),
    ] {
        Arc::make_mut(&mut request.headers).push(Header {
            name: name.into(),
            value: value.into(),
        });
    }
    let outgoing = prepare_gateway_upstream_request(&request, true, None)
        .expect("cacheable request should be rewritten");
    assert_eq!(
        outgoing
            .headers
            .iter()
            .filter(|header| {
                [
                    "if-modified-since",
                    "if-unmodified-since",
                    "if-none-match",
                    "if-match",
                    "range",
                    "if-range",
                ]
                .iter()
                .any(|name| header.name.eq_ignore_ascii_case(name))
            })
            .count(),
        0
    );
    let variables = Variables::new(&outgoing, "localhost", server, &local(18148), &remote());
    let params = build_fastcgi_params(&outgoing, &test_env(), context, &variables);
    assert_eq!(
        params
            .iter()
            .find(|(name, _)| name == "HTTP_IF_MODIFIED_SINCE")
            .map(|(_, value)| value.as_str()),
        Some("configured")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn dynamic_pass_resolves_a_named_upstream() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                upstream backend {
                    server 127.0.0.1:9000;
                }
                server {
                    listen 127.0.0.1:18149;
                    location / {
                        fastcgi_pass $arg_b;
                    }
                }
            }
        "#,
        &[],
    );
    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    let context = &server.locations[0].context;
    let request = plain_request("/?b=backend", "127.0.0.1:18149");
    let mut variables = Variables::new(&request, "localhost", server, &local(18149), &remote());
    variables.args = "b=backend".into();

    let targets = resolve_fastcgi_targets(
        context.fastcgi_pass.as_ref().unwrap(),
        http,
        &variables,
        None,
    );
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].host, "127.0.0.1");
    assert_eq!(targets[0].port, 9000);
    let _ = std::fs::remove_dir_all(prefix);
}
