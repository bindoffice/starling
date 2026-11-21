use super::*;

#[test]
fn compiles_gzip_static_values_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                gzip_static on;
                server {
                    listen 127.0.0.1:18128;
                    location /always/ { gzip_static always; }
                    location /off/ { gzip_static off; }
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
    assert_eq!(server.context.gzip_static, GzipStatic::On);
    assert_eq!(server.locations[0].context.gzip_static, GzipStatic::Always);
    assert_eq!(server.locations[1].context.gzip_static, GzipStatic::Off);

    fs::write(&config_path, "events {} http { gzip_static sometimes; }\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error.message.contains("invalid value \"sometimes\""),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_expires_forms_inheritance_and_validation() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                expires 1h;
                server {
                    listen 127.0.0.1:18471;
                    location /inherited { }
                    location /modified { expires modified 30m; }
                    location /daily { expires @15h30m; }
                    location /variable { expires $http_expires; }
                    location /conditional {
                        if ($arg_cache) { expires max; }
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
    assert!(matches!(
        server.context.expires.as_ref().map(|rule| &rule.value),
        Some(ExpiresValue::Access(ExpiresOffset::Fixed(3600)))
    ));
    assert!(matches!(
        server.locations[0]
            .context
            .expires
            .as_ref()
            .map(|rule| &rule.value),
        Some(ExpiresValue::Access(ExpiresOffset::Fixed(3600)))
    ));
    assert!(matches!(
        server.locations[1]
            .context
            .expires
            .as_ref()
            .map(|rule| &rule.value),
        Some(ExpiresValue::Modified(ExpiresOffset::Fixed(1800)))
    ));
    assert!(matches!(
        server.locations[2]
            .context
            .expires
            .as_ref()
            .map(|rule| &rule.value),
        Some(ExpiresValue::Access(ExpiresOffset::Daily(55_800)))
    ));
    assert!(matches!(
        server.locations[3]
            .context
            .expires
            .as_ref()
            .map(|rule| &rule.value),
        Some(ExpiresValue::Access(ExpiresOffset::Variable(_)))
    ));
    assert!(matches!(
        server.locations[4].context.conditionals[0]
            .expires
            .as_ref()
            .map(|rule| &rule.value),
        Some(ExpiresValue::Max)
    ));

    for (source, expected) in [
        (
            "events {} http { expires 1h; expires 2h; }",
            "\"expires\" directive is duplicate",
        ),
        ("events {} http { expires @25h; }", "invalid value \"@25h\""),
        ("events {} http { expires 1.5h; }", "invalid value \"1.5h\""),
        (
            "events {} http { expires modified @1h; }",
            "invalid value \"@1h\"",
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_if_modified_since_scope_inheritance_and_validation() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                if_modified_since before;
                server {
                    listen 127.0.0.1:18472;
                    location /inherited { }
                    location /off { if_modified_since off; }
                    location /exact { if_modified_since exact; }
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
    assert_eq!(server.context.if_modified_since, IfModifiedSince::Before);
    assert_eq!(
        server.locations[0].context.if_modified_since,
        IfModifiedSince::Before
    );
    assert_eq!(
        server.locations[1].context.if_modified_since,
        IfModifiedSince::Off
    );
    assert_eq!(
        server.locations[2].context.if_modified_since,
        IfModifiedSince::Exact
    );

    fs::write(
        &config_path,
        "events {} http { if_modified_since exact; if_modified_since before; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(
        error
            .message
            .contains("\"if_modified_since\" directive is duplicate"),
        "{error}"
    );

    fs::write(
        &config_path,
        "events {} http { if_modified_since sometimes; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("invalid value"), "{error}");
    let _ = fs::remove_dir_all(prefix);
}
