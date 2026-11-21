use super::*;

#[test]
fn compiles_upstream_buffering_defaults_and_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_buffering off;
                proxy_buffers 4 8k;
                proxy_buffer_size 8k;
                proxy_busy_buffers_size 16k;
                proxy_temp_path proxy-temp 1 2;
                proxy_max_temp_file_size 64m;
                proxy_temp_file_write_size 16k;
                fastcgi_buffers 16 4k;
                server {
                    listen 127.0.0.1:18579;
                    fastcgi_buffering off;
                    fastcgi_request_buffering off;
                    fastcgi_buffer_size 8k;
                    fastcgi_busy_buffers_size 8k;
                    fastcgi_max_temp_file_size 0;
                    location /inherited {
                        proxy_pass http://127.0.0.1:19000;
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /override {
                        proxy_buffering on;
                        proxy_buffers 8 16k;
                        proxy_buffer_size 16k;
                        proxy_busy_buffers_size 32k;
                        proxy_temp_path custom-proxy-temp 2;
                        proxy_max_temp_file_size 128m;
                        proxy_temp_file_write_size 32k;
                        uwsgi_buffer_size 12k;
                        uwsgi_buffers 4 12k;
                        uwsgi_busy_buffers_size 24k;
                        uwsgi_request_buffering off;
                        scgi_buffering off;
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

    let inherited = &server.locations[0].context;
    assert!(!inherited.proxy_buffering.enabled);
    assert_eq!(inherited.proxy_buffering.buffer_size, 8 * 1024);
    assert_eq!(inherited.proxy_buffering.buffers.count, 4);
    assert_eq!(inherited.proxy_buffering.buffers.size, 8 * 1024);
    assert_eq!(
        inherited.proxy_buffering.resolved_busy_buffers_size(),
        16 * 1024
    );
    assert_eq!(
        inherited.proxy_buffering.temp_path.path,
        prefix.join("proxy-temp")
    );
    assert_eq!(inherited.proxy_buffering.temp_path.levels, [1, 2, 0]);
    assert_eq!(
        inherited.proxy_buffering.resolved_max_temp_file_size(),
        64 * 1024 * 1024
    );
    assert_eq!(
        inherited.proxy_buffering.resolved_temp_file_write_size(),
        16 * 1024
    );

    assert!(!inherited.fastcgi_buffering.enabled);
    assert!(!inherited.fastcgi_request_buffering);
    assert!(inherited.uwsgi_request_buffering);
    assert!(inherited.scgi_request_buffering);
    assert_eq!(inherited.fastcgi_buffering.buffer_size, 8 * 1024);
    assert_eq!(inherited.fastcgi_buffering.buffers.count, 16);
    assert_eq!(inherited.fastcgi_buffering.buffers.size, 4 * 1024);
    assert_eq!(
        inherited.fastcgi_buffering.resolved_busy_buffers_size(),
        8 * 1024
    );
    assert_eq!(inherited.fastcgi_buffering.resolved_max_temp_file_size(), 0);
    assert_eq!(
        inherited.fastcgi_buffering.temp_path.path,
        prefix.join("fastcgi_temp")
    );

    let overridden = &server.locations[1].context;
    assert!(overridden.proxy_buffering.enabled);
    assert_eq!(overridden.proxy_buffering.buffer_size, 16 * 1024);
    assert_eq!(overridden.proxy_buffering.buffers.count, 8);
    assert_eq!(overridden.proxy_buffering.buffers.size, 16 * 1024);
    assert_eq!(
        overridden.proxy_buffering.resolved_busy_buffers_size(),
        32 * 1024
    );
    assert_eq!(
        overridden.proxy_buffering.temp_path.path,
        prefix.join("custom-proxy-temp")
    );
    assert_eq!(overridden.proxy_buffering.temp_path.levels, [2, 0, 0]);
    assert_eq!(
        overridden.proxy_buffering.resolved_max_temp_file_size(),
        128 * 1024 * 1024
    );
    assert_eq!(overridden.uwsgi_buffering.buffer_size, 12 * 1024);
    assert_eq!(overridden.uwsgi_buffering.buffers.count, 4);
    assert_eq!(overridden.uwsgi_buffering.buffers.size, 12 * 1024);
    assert_eq!(
        overridden.uwsgi_buffering.resolved_busy_buffers_size(),
        24 * 1024
    );
    assert!(!overridden.uwsgi_request_buffering);
    assert!(!overridden.scgi_buffering.enabled);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn uses_nginx_upstream_buffering_defaults() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18580; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;

    for (config, temp_name) in [
        (&context.proxy_buffering, "proxy_temp"),
        (&context.fastcgi_buffering, "fastcgi_temp"),
        (&context.uwsgi_buffering, "uwsgi_temp"),
        (&context.scgi_buffering, "scgi_temp"),
    ] {
        assert!(config.enabled);
        assert_eq!(config.buffer_size, 4096);
        assert_eq!(config.buffers.count, 8);
        assert_eq!(config.buffers.size, 4096);
        assert_eq!(config.resolved_busy_buffers_size(), 8192);
        assert_eq!(config.resolved_temp_file_write_size(), 8192);
        assert_eq!(config.resolved_max_temp_file_size(), 1024 * 1024 * 1024);
        assert_eq!(config.temp_path.path, prefix.join(temp_name));
        assert_eq!(config.temp_path.levels, [0, 0, 0]);
    }
    assert!(context.proxy_request_buffering);
    assert!(context.fastcgi_request_buffering);
    assert!(context.uwsgi_request_buffering);
    assert!(context.scgi_request_buffering);
    assert!(context.grpc.request_buffering);
    assert!(!context.proxy_ignore_client_abort);
    assert!(!context.fastcgi_ignore_client_abort);
    assert!(!context.uwsgi_ignore_client_abort);
    assert!(!context.scgi_ignore_client_abort);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_ignore_client_abort_scope_and_duplicate_rules() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_ignore_client_abort on;
                fastcgi_ignore_client_abort on;
                uwsgi_ignore_client_abort on;
                scgi_ignore_client_abort on;
                server {
                    listen 127.0.0.1:18582;
                    location /inherited {
                        proxy_pass http://127.0.0.1:19000;
                        fastcgi_pass 127.0.0.1:9000;
                    }
                    location /override {
                        proxy_ignore_client_abort off;
                        fastcgi_ignore_client_abort off;
                        uwsgi_ignore_client_abort off;
                        scgi_ignore_client_abort off;
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
    let inherited = &server.locations[0].context;
    assert!(inherited.proxy_ignore_client_abort);
    assert!(inherited.fastcgi_ignore_client_abort);
    assert!(inherited.uwsgi_ignore_client_abort);
    assert!(inherited.scgi_ignore_client_abort);
    let overridden = &server.locations[1].context;
    assert!(!overridden.proxy_ignore_client_abort);
    assert!(!overridden.fastcgi_ignore_client_abort);
    assert!(!overridden.uwsgi_ignore_client_abort);
    assert!(!overridden.scgi_ignore_client_abort);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_upstream_buffering_directives_like_nginx() {
    for prefix in ["proxy", "fastcgi", "uwsgi", "scgi"] {
        for (directive, expected) in [
            (
                format!("{prefix}_buffering invalid;"),
                format!("invalid value \"invalid\" in \"{prefix}_buffering\" directive"),
            ),
            (
                format!("{prefix}_buffer_size 0;"),
                format!("invalid value \"0\" in \"{prefix}_buffer_size\" directive"),
            ),
            (
                format!("{prefix}_buffers 1 4k;"),
                format!("there must be at least 2 \"{prefix}_buffers\""),
            ),
            (
                format!("{prefix}_buffers 4 4k; {prefix}_busy_buffers_size 2k;"),
                format!("\"{prefix}_busy_buffers_size\" must be equal to or greater than"),
            ),
            (
                format!("{prefix}_buffers 2 4k; {prefix}_busy_buffers_size 8k;"),
                format!("\"{prefix}_busy_buffers_size\" must be less than the size of all"),
            ),
            (
                format!("{prefix}_temp_file_write_size 1k;"),
                format!("\"{prefix}_temp_file_write_size\" must be equal to or greater"),
            ),
            (
                format!("{prefix}_max_temp_file_size 1k;"),
                format!("\"{prefix}_max_temp_file_size\" must be equal to zero to disable"),
            ),
            (
                format!("{prefix}_buffering on; {prefix}_buffering off;"),
                format!("\"{prefix}_buffering\" directive is duplicate"),
            ),
            (
                format!("{prefix}_request_buffering invalid;"),
                format!("invalid value \"invalid\" in \"{prefix}_request_buffering\" directive"),
            ),
            (
                format!("{prefix}_request_buffering on; {prefix}_request_buffering off;"),
                format!("\"{prefix}_request_buffering\" directive is duplicate"),
            ),
            (
                format!("{prefix}_ignore_client_abort invalid;"),
                format!("invalid value \"invalid\" in \"{prefix}_ignore_client_abort\" directive"),
            ),
            (
                format!("{prefix}_ignore_client_abort on; {prefix}_ignore_client_abort off;"),
                format!("\"{prefix}_ignore_client_abort\" directive is duplicate"),
            ),
            (
                format!("{prefix}_buffers 4;"),
                format!("invalid number of arguments in \"{prefix}_buffers\" directive"),
            ),
            (
                format!("{prefix}_temp_path path 0;"),
                format!("invalid value \"0\" in \"{prefix}_temp_path\" directive"),
            ),
        ] {
            let error = compile_error(&format!(
                "events {{}} http {{ server {{ listen 127.0.0.1:18581; {directive} }} }}"
            ));
            assert!(error.contains(&expected), "{prefix}: {error}");
        }
    }
}

fn compile_error(source: &str) -> String {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .unwrap_err()
        .to_string();
    let _ = fs::remove_dir_all(prefix);
    error
}
