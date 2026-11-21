use super::*;

#[test]
fn compiles_fastcgi_keep_conn_and_catch_stderr_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_keep_conn on;
                fastcgi_catch_stderr "http-level";
                server {
                    listen 127.0.0.1:18136;
                    fastcgi_catch_stderr "server-level";
                    location /inherited {
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /override {
                        fastcgi_keep_conn off;
                        fastcgi_catch_stderr "location-level";
                        fastcgi_catch_stderr "location-second";
                        fastcgi_pass 127.0.0.1:9000;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(server.context.fastcgi_keep_conn);
    assert_eq!(
        server.context.fastcgi_catch_stderr,
        vec!["server-level".to_string()]
    );
    assert!(server.locations[0].context.fastcgi_keep_conn);
    assert_eq!(
        server.locations[0].context.fastcgi_catch_stderr,
        vec!["server-level".to_string()]
    );
    assert!(!server.locations[1].context.fastcgi_keep_conn);
    assert_eq!(
        server.locations[1].context.fastcgi_catch_stderr,
        vec!["location-level".to_string(), "location-second".to_string()]
    );

    for source in [
        "events {} http { fastcgi_keep_conn maybe; }",
        "events {} http { fastcgi_catch_stderr; }",
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains("invalid value")
                || error.message.contains("invalid number of arguments"),
            "{error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_gateway_response_header_controls_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_hide_header X-Http;
                fastcgi_pass_header X-Http-Pass;
                fastcgi_ignore_headers X-Accel-Redirect Expires;
                server {
                    listen 127.0.0.1:18140;
                    fastcgi_hide_header X-Server;
                    fastcgi_pass_header X-Server-Pass;
                    fastcgi_ignore_headers Cache-Control;
                    uwsgi_hide_header U-Hide;
                    uwsgi_pass_header U-Pass;
                    uwsgi_ignore_headers Set-Cookie;
                    scgi_hide_header S-Hide;
                    scgi_pass_header S-Pass;
                    scgi_ignore_headers Vary;
                    location /base {
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /override {
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_hide_header X-Local;
                        fastcgi_pass_header X-Local-Pass;
                        fastcgi_ignore_headers X-Accel-Charset X-Accel-Redirect;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server.context.fastcgi_hide_headers,
        vec!["X-Server".to_string()]
    );
    assert_eq!(
        server.context.fastcgi_pass_headers,
        vec!["X-Server-Pass".to_string()]
    );
    assert_eq!(
        server.context.fastcgi_ignore_headers,
        vec!["Cache-Control".to_string()]
    );
    assert_eq!(server.context.uwsgi_hide_headers, vec!["U-Hide"]);
    assert_eq!(server.context.uwsgi_pass_headers, vec!["U-Pass"]);
    assert_eq!(
        server.context.uwsgi_ignore_headers,
        vec!["Set-Cookie".to_string()]
    );
    assert_eq!(server.context.scgi_hide_headers, vec!["S-Hide"]);
    assert_eq!(server.context.scgi_pass_headers, vec!["S-Pass"]);
    assert_eq!(server.context.scgi_ignore_headers, vec!["Vary".to_string()]);

    let inherited = &server.locations[0].context;
    assert_eq!(inherited.fastcgi_hide_headers, vec!["X-Server".to_string()]);
    let overridden = &server.locations[1].context;
    assert_eq!(overridden.fastcgi_hide_headers, vec!["X-Local".to_string()]);
    assert_eq!(
        overridden.fastcgi_pass_headers,
        vec!["X-Local-Pass".to_string()]
    );
    assert_eq!(
        overridden.fastcgi_ignore_headers,
        vec![
            "X-Accel-Charset".to_string(),
            "X-Accel-Redirect".to_string()
        ]
    );

    for source in [
        "events {} http { fastcgi_hide_header; }",
        "events {} http { fastcgi_pass_header X-One X-Two; }",
        "events {} http { fastcgi_ignore_headers; }",
        "events {} http { fastcgi_ignore_headers Unknown; }",
        "events {} http { uwsgi_hide_header; }",
        "events {} http { uwsgi_ignore_headers Unknown; }",
        "events {} http { scgi_pass_header X-One X-Two; }",
        "events {} http { scgi_ignore_headers Unknown; }",
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains("invalid value")
                || error.message.contains("invalid number of arguments"),
            "{error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_proxy_ignore_headers_with_scope_override() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_ignore_headers X-Accel-Redirect Expires;
                server {
                    listen 127.0.0.1:18143;
                    proxy_ignore_headers Cache-Control;
                    location / {
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
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server.context.proxy_ignore_headers,
        vec!["Cache-Control".to_string()]
    );
    assert_eq!(
        server.locations[0].context.proxy_ignore_headers,
        vec!["Cache-Control".to_string()]
    );

    for source in [
        "events {} http { proxy_ignore_headers; }",
        "events {} http { proxy_ignore_headers Unknown; }",
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains("invalid value")
                || error.message.contains("invalid number of arguments"),
            "{error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
