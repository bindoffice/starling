use super::*;

#[test]
fn validates_proxy_redirect_configuration_order_and_duplicates() {
    let cases = [
        (
            "http { server { location / { proxy_redirect default; proxy_pass http://127.0.0.1/; } } }",
            "should be placed after",
        ),
        (
            "http { server { location / { proxy_pass http://$host/; proxy_redirect default; } } }",
            "cannot be used with \"proxy_pass\" directive with variables",
        ),
        (
            "http { server { location / { proxy_pass http://127.0.0.1/; proxy_redirect off; proxy_redirect off; } } }",
            "directive is duplicate",
        ),
    ];
    for (source, expected) in cases {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(&config_path, format!("events {{}}\n{source}\n")).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error: {}",
            error.message
        );
        let _ = fs::remove_dir_all(prefix);
    }
}

#[test]
fn compiles_redirect_location_controls() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                absolute_redirect off;
                server {
                    listen 127.0.0.1:18134;
                    server_name example.test;
                    port_in_redirect off;
                    location / {
                        server_name_in_redirect on;
                        return 302 /next;
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
    assert!(!server.context.absolute_redirect);
    assert!(!server.context.port_in_redirect);
    assert!(!server.context.server_name_in_redirect);
    let location = &server.locations[0].context;
    assert!(!location.absolute_redirect);
    assert!(!location.port_in_redirect);
    assert!(location.server_name_in_redirect);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_unix_stream_upstreams() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            stream {
                upstream local_app {
                    server unix:/run/starling-app.sock;
                }
                server {
                    listen 127.0.0.1:18135;
                    proxy_pass local_app;
                }
                server {
                    listen 127.0.0.1:18136;
                    proxy_pass unix:/run/starling-direct.sock;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let stream = config.stream.as_ref().unwrap();
    assert_eq!(
        stream.upstreams["local_app"].servers[0].address,
        "unix:/run/starling-app.sock"
    );
    assert_eq!(
        stream.servers[0]
            .proxy_pass
            .as_ref()
            .unwrap()
            .upstream
            .as_deref(),
        Some("local_app")
    );
    assert_eq!(
        stream.servers[1].proxy_pass.as_ref().unwrap().target,
        "unix:/run/starling-direct.sock"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_unix_http_proxy_targets() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream local_app {
                    server unix:/run/starling-http.sock;
                }
                server {
                    listen 127.0.0.1:18142;
                    location /direct/ {
                        proxy_pass http://unix:/run/starling-direct.sock:/api/;
                    }
                    location /named/ {
                        proxy_pass http://local_app;
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
    let http = config.http.as_ref().unwrap();
    let direct = http.servers[0].locations[0]
        .context
        .proxy_pass
        .as_ref()
        .unwrap();
    assert_eq!(direct.host, "unix:/run/starling-direct.sock");
    assert_eq!(direct.port, 0);
    assert_eq!(direct.uri.as_deref(), Some("/api/"));
    let named = http.servers[0].locations[1]
        .context
        .proxy_pass
        .as_ref()
        .unwrap();
    assert_eq!(named.upstream.as_deref(), Some("local_app"));
    assert_eq!(
        http.upstreams["local_app"].servers[0].address,
        "unix:/run/starling-http.sock"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn resolves_relative_unix_listen_paths_against_the_prefix() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen unix:logs/http.sock;
                    location / { return 204; }
                }
            }
            stream {
                server {
                    listen unix:logs/stream.sock;
                    proxy_pass 127.0.0.1:19000;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].listens[0]
            .address
            .unix_path(),
        Some(prefix.join("logs/http.sock").as_path())
    );
    assert_eq!(
        config.stream.as_ref().unwrap().servers[0].listens[0]
            .address
            .unix_path(),
        Some(prefix.join("logs/stream.sock").as_path())
    );
    assert!(matches!(
        config.stream.as_ref().unwrap().servers[0].listens[0].address,
        ListenAddress::Unix(_)
    ));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_udp_on_unix_stream_listen_sockets() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            stream {
                server {
                    listen unix:logs/stream.sock udp;
                    proxy_pass 127.0.0.1:19000;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("incompatible with UNIX"), "{error}");
    let _ = fs::remove_dir_all(prefix);
}
