use super::*;

#[test]
fn conditionals_switch_proxy_and_grpc_backends() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18573;
                    location /if-grpc {
                        proxy_pass http://127.0.0.1:19000;
                        if ($http_x_mode = grpc) {
                            grpc_pass grpc://127.0.0.1:19001;
                        }
                    }
                    location /if-proxy {
                        grpc_pass grpc://127.0.0.1:19002;
                        if ($http_x_mode = proxy) {
                            proxy_pass http://127.0.0.1:19003;
                        }
                    }
                    location /if-reset {
                        proxy_pass http://127.0.0.1:19008;
                        if ($http_x_first = yes) {
                            proxy_pass http://127.0.0.1:19009;
                        }
                        if ($http_x_second = yes) {
                        }
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let if_grpc = apply_conditional(&server.locations[0], server, "grpc");
    assert!(if_grpc.proxy_pass.is_none());
    assert_eq!(
        if_grpc.grpc.pass.as_ref().map(|pass| pass.port),
        Some(19001)
    );

    let if_proxy = apply_conditional(&server.locations[1], server, "proxy");
    assert_eq!(
        if_proxy.proxy_pass.as_ref().map(|pass| pass.port),
        Some(19003)
    );
    assert!(if_proxy.grpc.pass.is_none());

    let first_only = apply_conditional_headers(&server.locations[2], server, &[("X-First", "yes")]);
    assert_eq!(
        first_only.proxy_pass.as_ref().map(|pass| pass.port),
        Some(19009)
    );
    let reset = apply_conditional_headers(
        &server.locations[2],
        server,
        &[("X-First", "yes"), ("X-Second", "yes")],
    );
    assert_eq!(reset.proxy_pass.as_ref().map(|pass| pass.port), Some(19008));
    let second_only =
        apply_conditional_headers(&server.locations[2], server, &[("X-Second", "yes")]);
    assert_eq!(
        second_only.proxy_pass.as_ref().map(|pass| pass.port),
        Some(19008)
    );

    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn conditionals_snapshot_and_restore_gateway_passes() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18574;
                    location /if-fastcgi {
                        fastcgi_pass 127.0.0.1:19010;
                        if ($http_x_uwsgi = yes) {
                            uwsgi_pass 127.0.0.1:19011;
                        }
                        if ($http_x_reset = yes) {
                        }
                    }
                    location /if-scgi {
                        scgi_pass 127.0.0.1:19012;
                        if ($http_x_memcached = yes) {
                            memcached_pass 127.0.0.1:19013;
                        }
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];

    let switched = apply_conditional_headers(&server.locations[0], server, &[("X-Uwsgi", "yes")]);
    assert!(switched.fastcgi_pass.is_none());
    assert_eq!(
        switched.uwsgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19011")
    );

    let reset = apply_conditional_headers(
        &server.locations[0],
        server,
        &[("X-Uwsgi", "yes"), ("X-Reset", "yes")],
    );
    assert_eq!(
        reset.fastcgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19010")
    );
    assert!(reset.uwsgi_pass.is_none());

    let memcached =
        apply_conditional_headers(&server.locations[1], server, &[("X-Memcached", "yes")]);
    assert!(memcached.scgi_pass.is_none());
    assert_eq!(
        memcached
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:19013")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn empty_limit_except_preserves_parent_handler() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18575;
                    location /limited {
                        grpc_pass grpc://127.0.0.1:19020;
                        limit_except GET {
                            deny all;
                        }
                    }
                }
            }
        "#,
        &[],
    );
    let location = &config.http.as_ref().unwrap().servers[0].locations[0];
    let mut context = location.context.clone();
    let request = request_with_headers("POST", "/limited", &[]);
    apply_limit_except(&mut context, &request);
    assert_eq!(
        context.grpc.pass.as_ref().map(|pass| pass.port),
        Some(19020)
    );
    assert!(context.proxy_pass.is_none());
    let _ = std::fs::remove_dir_all(prefix);
}

fn apply_conditional(
    location: &LocationConfig,
    server: &ServerConfig,
    mode: &str,
) -> Arc<EffectiveConfig> {
    apply_conditional_headers(location, server, &[("X-Mode", mode)])
}

fn apply_conditional_headers(
    location: &LocationConfig,
    server: &ServerConfig,
    headers: &[(&str, &str)],
) -> Arc<EffectiveConfig> {
    let request = request_with_headers("GET", "/conditional", headers);
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18573".parse().unwrap();
    let mut variables = Variables::new(&request, "localhost", server, &local, &remote);
    let mut env = request_env("/conditional");
    let mut context = location.context.clone();
    let conditionals = context.conditionals.clone();
    let outcome = apply_conditionals(
        &conditionals,
        &mut env,
        Arc::make_mut(&mut context),
        &mut variables,
        &request,
    );
    assert!(matches!(outcome, ConditionalOutcome::None));
    context
}

fn request_with_headers(method: &str, target: &str, headers: &[(&str, &str)]) -> Request {
    Request {
        method: method.into(),
        target: target.into(),
        version: "HTTP/1.1".into(),
        headers: (headers
            .iter()
            .map(|(name, value)| Header {
                name: (*name).into(),
                value: (*value).into(),
            })
            .collect::<Vec<_>>())
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18573".into(),
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
