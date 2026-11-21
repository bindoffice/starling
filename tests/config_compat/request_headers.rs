use super::*;

#[test]
fn compiles_request_header_limits_and_policy_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                client_header_buffer_size 2k;
                large_client_header_buffers 8 16k;
                ignore_invalid_headers off;
                underscores_in_headers on;
                server {
                    listen 127.0.0.1:18481;
                    location /inherited {}
                }
                server {
                    listen 127.0.0.1:18482;
                    client_header_buffer_size 4k;
                    large_client_header_buffers 2 32k;
                    ignore_invalid_headers on;
                    underscores_in_headers off;
                    location /override {}
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let http = config.http.as_ref().unwrap();

    assert_eq!(http.servers[0].context.client_header_buffer_size, 2048);
    assert_eq!(
        http.servers[0].context.large_client_header_buffers,
        starling::config::LargeClientHeaderBuffers {
            number: 8,
            size: 16 * 1024,
        }
    );
    assert!(!http.servers[0].context.ignore_invalid_headers);
    assert!(http.servers[0].context.underscores_in_headers);
    assert_eq!(
        http.servers[0].locations[0]
            .context
            .client_header_buffer_size,
        2048
    );
    assert_eq!(http.servers[1].context.client_header_buffer_size, 4096);
    assert_eq!(
        http.servers[1].context.large_client_header_buffers,
        starling::config::LargeClientHeaderBuffers {
            number: 2,
            size: 32 * 1024,
        }
    );
    assert!(http.servers[1].context.ignore_invalid_headers);
    assert!(!http.servers[1].context.underscores_in_headers);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn uses_nginx_request_header_defaults() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18483; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.client_header_buffer_size, 1024);
    assert_eq!(
        context.large_client_header_buffers,
        starling::config::LargeClientHeaderBuffers {
            number: 4,
            size: 8 * 1024,
        }
    );
    assert!(context.ignore_invalid_headers);
    assert!(!context.underscores_in_headers);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_or_misplaced_request_header_directives() {
    for (directive, expected) in [
        (
            "client_header_buffer_size 0;",
            "invalid value \"0\" in \"client_header_buffer_size\" directive",
        ),
        ("client_header_buffer_size nope;", "invalid size \"nope\""),
        (
            "large_client_header_buffers 0 8k;",
            "invalid value \"0 8k\" in \"large_client_header_buffers\" directive",
        ),
        (
            "large_client_header_buffers 4 0;",
            "invalid value \"4 0\" in \"large_client_header_buffers\" directive",
        ),
        (
            "large_client_header_buffers 4;",
            "invalid number of arguments in \"large_client_header_buffers\" directive",
        ),
        (
            "ignore_invalid_headers maybe;",
            "invalid value \"maybe\" in \"ignore_invalid_headers\" directive",
        ),
        (
            "ignore_invalid_headers on; ignore_invalid_headers off;",
            "\"ignore_invalid_headers\" directive is duplicate",
        ),
        (
            "server { listen 127.0.0.1:18484; location / { ignore_invalid_headers off; } }",
            "\"ignore_invalid_headers\" directive is not allowed here",
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
