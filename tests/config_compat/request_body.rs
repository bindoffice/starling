use super::*;

#[test]
fn compiles_request_body_storage_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                client_body_buffer_size 16k;
                client_body_temp_path body_temp 1 2;
                client_body_in_file_only clean;
                client_body_in_single_buffer on;
                server {
                    listen 127.0.0.1:18491;
                    location /inherited {}
                    location /override {
                        client_body_buffer_size 32k;
                        client_body_temp_path /tmp/starling-body-override;
                        client_body_in_file_only on;
                        client_body_in_single_buffer off;
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

    assert_eq!(server.context.client_body.buffer_size, 16 * 1024);
    assert_eq!(
        server.context.client_body.temp_path.path,
        prefix.join("body_temp")
    );
    assert_eq!(server.context.client_body.temp_path.levels, [1, 2, 0]);
    assert_eq!(
        server.context.client_body.in_file_only,
        starling::config::ClientBodyInFileOnly::Clean
    );
    assert!(server.context.client_body.in_single_buffer);

    let inherited = &server.locations[0].context;
    assert_eq!(inherited.client_body.buffer_size, 16 * 1024);
    assert_eq!(
        inherited.client_body.temp_path.path,
        prefix.join("body_temp")
    );

    let override_context = &server.locations[1].context;
    assert_eq!(override_context.client_body.buffer_size, 32 * 1024);
    assert_eq!(
        override_context.client_body.temp_path.path,
        PathBuf::from("/tmp/starling-body-override")
    );
    assert_eq!(
        override_context.client_body.in_file_only,
        starling::config::ClientBodyInFileOnly::On
    );
    assert!(!override_context.client_body.in_single_buffer);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn uses_nginx_request_body_defaults() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18492; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.client_body.buffer_size, 2 * 4096);
    assert_eq!(
        context.client_body.temp_path.path,
        prefix.join("client_body_temp")
    );
    assert_eq!(context.client_body.temp_path.levels, [0, 0, 0]);
    assert_eq!(
        context.client_body.in_file_only,
        starling::config::ClientBodyInFileOnly::Off
    );
    assert!(!context.client_body.in_single_buffer);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_or_duplicate_request_body_directives() {
    for (directive, expected) in [
        (
            "client_body_buffer_size 0;",
            "invalid value \"0\" in \"client_body_buffer_size\" directive",
        ),
        ("client_body_buffer_size nope;", "invalid size \"nope\""),
        (
            "client_body_buffer_size 1k; client_body_buffer_size 2k;",
            "\"client_body_buffer_size\" directive is duplicate",
        ),
        (
            "client_body_temp_path /tmp/body 0;",
            "invalid value \"0\" in \"client_body_temp_path\" directive",
        ),
        (
            "client_body_temp_path /tmp/body nope;",
            "invalid value \"nope\" in \"client_body_temp_path\" directive",
        ),
        (
            "client_body_temp_path /tmp/body 11;",
            "invalid value in \"client_body_temp_path\" directive",
        ),
        (
            "client_body_temp_path /tmp/body 18446744073709551615;",
            "invalid value in \"client_body_temp_path\" directive",
        ),
        (
            "client_body_temp_path /tmp/body 1 1 1 1;",
            "invalid number of arguments in \"client_body_temp_path\" directive",
        ),
        (
            "client_body_in_file_only maybe;",
            "invalid value \"maybe\" in \"client_body_in_file_only\" directive",
        ),
        (
            "client_body_in_single_buffer maybe;",
            "invalid value \"maybe\" in \"client_body_in_single_buffer\" directive",
        ),
    ] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {directive}: {}",
            error.message
        );
        let _ = fs::remove_dir_all(prefix);
    }
}
