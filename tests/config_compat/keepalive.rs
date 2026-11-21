use super::*;

#[test]
fn compiles_keepalive_header_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                keepalive_timeout 30s 12s;
                server {
                    listen 127.0.0.1:18517;
                    location /inherited {}
                    location /timeout-only {
                        keepalive_timeout 20s;
                    }
                    location /disabled-header {
                        keepalive_timeout 10s 0s;
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
        server.context.keepalive_header,
        Some(Duration::from_secs(12))
    );
    assert_eq!(
        server.locations[0].context.keepalive_header,
        Some(Duration::from_secs(12))
    );
    assert_eq!(
        server.locations[1].context.keepalive_header,
        Some(Duration::from_secs(12))
    );
    assert_eq!(
        server.locations[1].context.keepalive_timeout,
        Duration::from_secs(20)
    );
    assert_eq!(
        server.locations[2].context.keepalive_header,
        Some(Duration::ZERO)
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_keepalive_min_timeout_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                keepalive_min_timeout 5s;
                server {
                    listen 127.0.0.1:18516;
                    location /inherited {}
                    location /short {
                        keepalive_min_timeout 750ms;
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
    assert_eq!(server.context.keepalive_min_timeout, Duration::from_secs(5));
    assert_eq!(
        server.locations[0].context.keepalive_min_timeout,
        Duration::from_secs(5)
    );
    assert_eq!(
        server.locations[1].context.keepalive_min_timeout,
        Duration::from_millis(750)
    );

    fs::write(
        &config_path,
        "events {} http { keepalive_min_timeout 1s; keepalive_min_timeout 2s; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("\"keepalive_min_timeout\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_keepalive_disable_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                keepalive_disable none;
                server {
                    listen 127.0.0.1:18515;
                    location /inherit {}
                    location /msie6 {
                        keepalive_disable msie6;
                    }
                    location /both {
                        keepalive_disable msie6;
                        keepalive_disable safari;
                    }
                    location /none {
                        keepalive_disable none;
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
    assert!(!server.context.keepalive_disable.msie6);
    assert!(!server.context.keepalive_disable.safari);
    assert!(!server.locations[0].context.keepalive_disable.msie6);
    assert!(server.locations[1].context.keepalive_disable.msie6);
    assert!(!server.locations[1].context.keepalive_disable.safari);
    assert!(server.locations[2].context.keepalive_disable.msie6);
    assert!(server.locations[2].context.keepalive_disable.safari);
    assert!(!server.locations[3].context.keepalive_disable.msie6);
    assert!(!server.locations[3].context.keepalive_disable.safari);

    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18515; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let disabled = config.http.as_ref().unwrap().servers[0]
        .context
        .keepalive_disable;
    assert!(disabled.msie6);
    assert!(!disabled.safari);

    for (directive, expected) in [
        (
            "keepalive_disable chrome;",
            "invalid value \"chrome\" in \"keepalive_disable\" directive",
        ),
        (
            "keepalive_disable msie6 safari none;",
            "invalid number of arguments in \"keepalive_disable\" directive",
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
        assert!(
            error.message.contains(expected),
            "unexpected error for {directive}: {error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
