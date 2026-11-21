use super::*;

#[test]
fn compiles_tunnel_pass_and_upstream_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                tunnel_bind 127.0.0.1;
                tunnel_socket_keepalive on;
                tunnel_socket_rcvbuf 32k;
                tunnel_socket_sndbuf 64k;
                tunnel_connect_timeout 2s;
                tunnel_send_timeout 3s;
                tunnel_read_timeout 4s;
                tunnel_send_lowat 2k;
                tunnel_buffer_size 8k;
                tunnel_next_upstream error timeout;
                tunnel_next_upstream_tries 2;
                tunnel_next_upstream_timeout 5s;
                server {
                    listen 127.0.0.1:18080;
                    location / {
                        tunnel_pass;
                    }
                    location /explicit {
                        tunnel_pass 127.0.0.1:8443;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let locations = &config.http.as_ref().unwrap().servers[0].locations;

    let inherited = &locations[0].context;
    assert_eq!(
        inherited.tunnel.pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("$host:$request_port")
    );
    assert_eq!(inherited.tunnel.bind.as_deref(), Some("127.0.0.1"));
    assert!(inherited.tunnel.socket_keepalive);
    assert_eq!(inherited.tunnel.socket_rcvbuf, 32 * 1024);
    assert_eq!(inherited.tunnel.socket_sndbuf, 64 * 1024);
    assert_eq!(inherited.tunnel.connect_timeout, Duration::from_secs(2));
    assert_eq!(inherited.tunnel.send_timeout, Duration::from_secs(3));
    assert_eq!(inherited.tunnel.read_timeout, Duration::from_secs(4));
    assert_eq!(inherited.tunnel.send_lowat, 2 * 1024);
    assert_eq!(inherited.tunnel.buffer_size, 8 * 1024);
    assert!(inherited.tunnel.next_upstream.retry_error());
    assert!(inherited.tunnel.next_upstream.retry_timeout());
    assert_eq!(inherited.tunnel.next_upstream.tries, 2);
    assert_eq!(
        inherited.tunnel.next_upstream.timeout,
        Duration::from_secs(5)
    );
    assert!(config.warnings.iter().any(|warning| {
        warning.message.contains("\"tunnel_send_lowat\"") && warning.message.contains("not applied")
    }));

    let explicit = &locations[1].context;
    assert_eq!(
        explicit.tunnel.pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:8443")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn tunnel_pass_switches_the_content_handler() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18081;
                    location /tunnel-last {
                        proxy_pass http://127.0.0.1:9000;
                        tunnel_pass 127.0.0.1:9001;
                    }
                    location /proxy-last {
                        tunnel_pass 127.0.0.1:9001;
                        proxy_pass http://127.0.0.1:9000;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let locations = &config.http.as_ref().unwrap().servers[0].locations;

    assert_eq!(
        locations[0]
            .context
            .tunnel
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:9001")
    );
    assert!(locations[0].context.proxy_pass.is_none());

    assert_eq!(
        locations[1]
            .context
            .proxy_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("http://127.0.0.1:9000")
    );
    assert!(locations[1].context.tunnel.pass.is_none());
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn server_tunnel_pass_is_inherited_and_location_handlers_override_it() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream tunnel_backend {
                    server 127.0.0.1:9443;
                }
                server {
                    listen 127.0.0.1:18087;
                    tunnel_pass tunnel_backend;

                    location /inherited {}

                    location /proxy {
                        proxy_pass http://127.0.0.1:9000;
                    }

                    location /nested {
                        location /nested/child {}
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server
            .context
            .tunnel
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("tunnel_backend")
    );

    let inherited = &server.locations[0];
    assert_eq!(
        inherited
            .context
            .tunnel
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("tunnel_backend")
    );

    let proxied = &server.locations[1];
    assert!(proxied.context.tunnel.pass.is_none());
    assert!(proxied.context.proxy_pass.is_some());

    let nested = &server.locations[2].nested[0];
    assert_eq!(
        nested
            .context
            .tunnel
            .pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("tunnel_backend")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn tunnel_next_upstream_off_disables_retries() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18082;
                    location / {
                        tunnel_next_upstream off;
                        tunnel_next_upstream_tries 3;
                        tunnel_pass 127.0.0.1:9001;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let tunnel = &config.http.as_ref().unwrap().servers[0].locations[0]
        .context
        .tunnel;
    assert!(!tunnel.next_upstream.retry_error());
    assert!(!tunnel.next_upstream.retry_timeout());
    assert_eq!(tunnel.next_upstream.tries, 3);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_tunnel_pass_inside_if() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18086;
                    location / {
                        if ($arg_tunnel = yes) {
                            tunnel_pass 127.0.0.1:9443;
                        }
                        return 204;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let actions = &config.http.as_ref().unwrap().servers[0].locations[0]
        .context
        .rewrite_actions;
    let tunnel = actions.iter().find_map(|action| match action {
        starling::config::RewriteAction::If(conditional) => conditional.tunnel_pass.as_ref(),
        _ => None,
    });
    assert_eq!(tunnel.map(|pass| pass.raw.as_str()), Some("127.0.0.1:9443"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_tunnel_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18083; location / { tunnel_pass one two; } } }",
            "invalid number of arguments",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18084; location / { tunnel_pass one; tunnel_pass two; } } }",
            "duplicate",
        ),
        (
            "events {} http { tunnel_connect_timeout invalid; }",
            "invalid time value",
        ),
        (
            "events {} http { tunnel_buffer_size 0; }",
            "invalid value \"0\"",
        ),
        (
            "events {} http { tunnel_next_upstream http_500; }",
            "invalid parameter",
        ),
        (
            "events {} http { tunnel_socket_keepalive maybe; }",
            "invalid value \"maybe\"",
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone())
            .expect_err("configuration should fail")
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}
