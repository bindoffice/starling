use super::*;

#[test]
fn compiles_http_proxy_bind_and_socket_options_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_bind 127.0.0.1;
                proxy_socket_keepalive on;
                proxy_socket_rcvbuf 32k;
                proxy_socket_sndbuf 64k;
                server {
                    listen 127.0.0.1:18133;
                    location /inherited {
                        proxy_pass http://127.0.0.1:19000;
                    }
                    location /override {
                        proxy_bind $remote_addr;
                        proxy_socket_keepalive off;
                        proxy_socket_rcvbuf 16k;
                        proxy_socket_sndbuf 128k;
                        proxy_pass http://127.0.0.1:19000;
                    }
                }
                server {
                    listen 127.0.0.1:18134;
                    proxy_bind off;
                    location / {
                        proxy_pass http://127.0.0.1:19000;
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
    let servers = &config.http.as_ref().unwrap().servers;

    let inherited = &servers[0].locations[0].context;
    assert_eq!(inherited.proxy_bind.as_deref(), Some("127.0.0.1"));
    assert!(inherited.proxy_socket_keepalive);
    assert_eq!(inherited.proxy_socket_rcvbuf, 32 * 1024);
    assert_eq!(inherited.proxy_socket_sndbuf, 64 * 1024);

    let overridden = &servers[0].locations[1].context;
    assert_eq!(overridden.proxy_bind.as_deref(), Some("$remote_addr"));
    assert!(!overridden.proxy_socket_keepalive);
    assert_eq!(overridden.proxy_socket_rcvbuf, 16 * 1024);
    assert_eq!(overridden.proxy_socket_sndbuf, 128 * 1024);

    assert_eq!(servers[1].locations[0].context.proxy_bind, None);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_http_proxy_bind_and_socket_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (directive, expected) in [
        (
            "proxy_bind 127.0.0.1 transparent;",
            "transparent proxying is not supported",
        ),
        (
            "proxy_bind 127.0.0.1 invalid;",
            "invalid parameter \"invalid\"",
        ),
        (
            "proxy_bind 127.0.0.1 one two;",
            "invalid number of arguments",
        ),
        (
            "proxy_bind not-an-address;",
            "invalid address \"not-an-address\"",
        ),
        ("proxy_bind 127.0.0.1:0;", "invalid address \"127.0.0.1:0\""),
        ("proxy_socket_rcvbuf invalid;", "invalid value \"invalid\""),
        ("proxy_socket_sndbuf invalid;", "invalid value \"invalid\""),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_fastcgi_bind_and_socket_options_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_bind 127.0.0.1;
                fastcgi_socket_keepalive on;
                fastcgi_socket_rcvbuf 32k;
                fastcgi_socket_sndbuf 64k;
                server {
                    listen 127.0.0.1:18139;
                    location /inherited {
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /override {
                        fastcgi_bind $remote_addr;
                        fastcgi_socket_keepalive off;
                        fastcgi_socket_rcvbuf 16k;
                        fastcgi_socket_sndbuf 128k;
                        fastcgi_pass 127.0.0.1:9000;
                    }
                }
                server {
                    listen 127.0.0.1:18140;
                    fastcgi_bind off;
                    location / {
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
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let servers = &config.http.as_ref().unwrap().servers;

    let inherited = &servers[0].locations[0].context;
    assert_eq!(inherited.fastcgi_bind.as_deref(), Some("127.0.0.1"));
    assert!(inherited.fastcgi_socket_keepalive);
    assert_eq!(inherited.fastcgi_socket_rcvbuf, 32 * 1024);
    assert_eq!(inherited.fastcgi_socket_sndbuf, 64 * 1024);

    let overridden = &servers[0].locations[1].context;
    assert_eq!(overridden.fastcgi_bind.as_deref(), Some("$remote_addr"));
    assert!(!overridden.fastcgi_socket_keepalive);
    assert_eq!(overridden.fastcgi_socket_rcvbuf, 16 * 1024);
    assert_eq!(overridden.fastcgi_socket_sndbuf, 128 * 1024);

    assert_eq!(servers[1].locations[0].context.fastcgi_bind, None);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_fastcgi_bind_and_socket_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (directive, expected) in [
        (
            "fastcgi_bind 127.0.0.1 transparent;",
            "transparent proxying is not supported",
        ),
        (
            "fastcgi_bind 127.0.0.1 invalid;",
            "invalid parameter \"invalid\"",
        ),
        (
            "fastcgi_bind 127.0.0.1 one two;",
            "invalid number of arguments",
        ),
        (
            "fastcgi_bind not-an-address;",
            "invalid address \"not-an-address\"",
        ),
        ("fastcgi_socket_keepalive maybe;", "invalid value \"maybe\""),
        (
            "fastcgi_socket_rcvbuf invalid;",
            "invalid value \"invalid\"",
        ),
        (
            "fastcgi_socket_sndbuf invalid;",
            "invalid value \"invalid\"",
        ),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_uwsgi_and_scgi_bind_and_socket_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                uwsgi_bind 127.0.0.1;
                uwsgi_socket_keepalive on;
                uwsgi_socket_rcvbuf 32k;
                uwsgi_socket_sndbuf 64k;
                scgi_bind 127.0.0.1;
                scgi_socket_keepalive on;
                scgi_socket_rcvbuf 16k;
                scgi_socket_sndbuf 128k;
                server {
                    listen 127.0.0.1:18142;
                    location /u {
                        uwsgi_pass 127.0.0.1:9001;
                    }
                    location /s {
                        scgi_pass 127.0.0.1:9002;
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
        locations[0].context.uwsgi_bind.as_deref(),
        Some("127.0.0.1")
    );
    assert!(locations[0].context.uwsgi_socket_keepalive);
    assert_eq!(locations[0].context.uwsgi_socket_rcvbuf, 32 * 1024);
    assert_eq!(locations[0].context.uwsgi_socket_sndbuf, 64 * 1024);
    assert_eq!(locations[1].context.scgi_bind.as_deref(), Some("127.0.0.1"));
    assert!(locations[1].context.scgi_socket_keepalive);
    assert_eq!(locations[1].context.scgi_socket_rcvbuf, 16 * 1024);
    assert_eq!(locations[1].context.scgi_socket_sndbuf, 128 * 1024);
    let _ = fs::remove_dir_all(prefix);
}
