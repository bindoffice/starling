use super::*;

#[test]
fn bundled_nginx_configuration_compiles() {
    let prefix = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config_path = prefix.join("conf/nginx.conf");
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .expect("load bundled configuration");
    let config =
        compile(document, prefix.clone(), config_path).expect("compile bundled configuration");
    let http = config.http.expect("http context");
    assert_eq!(http.servers.len(), 1);
    assert_eq!(
        http.mime_types.get("html").map(String::as_str),
        Some("text/html")
    );
    assert_eq!(http.servers[0].listens[0].address.port(), 80);
    assert_eq!(http.servers[0].locations.len(), 2);
}

#[test]
fn include_is_relative_to_the_configuration_file() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf/conf.d")).unwrap();
    fs::write(
        prefix.join("conf/nginx.conf"),
        "events {} http { include conf.d/*.conf; }\n",
    )
    .unwrap();
    fs::write(
        prefix.join("conf/conf.d/site.conf"),
        "server { listen 127.0.0.1:18090; location / { return 204; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&prefix.join("conf/nginx.conf"))
        .expect("load include");
    let config =
        compile(document, prefix.clone(), prefix.join("conf/nginx.conf")).expect("compile include");
    assert_eq!(config.http.unwrap().servers.len(), 1);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn accepts_core_file_and_socket_tuning_directives() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                subrequest_output_buffer_size 8k;
                send_lowat 1024;
                open_file_cache_events on;
                server {
                    listen 127.0.0.1:18212;
                    location / {
                        subrequest_output_buffer_size 16k;
                        send_lowat 2048;
                        open_file_cache_events off;
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
    let config = compile(document, prefix.clone(), config_path).expect("tuning directives compile");
    assert_eq!(
        config
            .warnings
            .iter()
            .filter(|warning| {
                warning.message.contains("\"send_lowat\"")
                    && warning.message.contains("not applied")
            })
            .count(),
        2,
        "both send_lowat scopes should emit actionable ignored warnings"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_worker_shutdown_timeout_and_rejects_invalid_values() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, "worker_shutdown_timeout 1500ms;\nevents {}\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert_eq!(
        config.core.worker_shutdown_timeout,
        Some(Duration::from_millis(1500))
    );

    for (source, expected) in [
        ("worker_shutdown_timeout forever;", "invalid time value"),
        (
            "worker_shutdown_timeout 1s;\nworker_shutdown_timeout 2s;",
            "directive is duplicate",
        ),
        (
            "worker_shutdown_timeout 1s 2s;",
            "invalid number of arguments",
        ),
    ] {
        fs::write(&config_path, format!("{source}\nevents {{}}\n")).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {source}: {}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn reports_unknown_directives_with_a_source_location() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    fs::write(
        prefix.join("conf/nginx.conf"),
        "events {}\nhttp { made_up_directive on; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&prefix.join("conf/nginx.conf"))
        .unwrap();
    let error = compile(document, prefix.clone(), prefix.join("conf/nginx.conf")).unwrap_err();
    assert!(error.message.contains("unknown directive"));
    assert_eq!(
        error.location.as_ref().map(|location| location.line),
        Some(2)
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_multiple_error_logs_and_syslog_targets() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            error_log logs/first.log info;
            error_log syslog:server=127.0.0.1:5514,facility=local0,tag=starling,nohostname warn;
            events {}
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();

    assert_eq!(config.core.error_logs.len(), 2);
    assert_eq!(config.core.error_logs[0].level, LogLevel::Info);
    assert!(matches!(
        &config.core.error_logs[0].destination,
        LogDestination::File(path) if path == &prefix.join("logs/first.log")
    ));
    assert_eq!(config.core.error_logs[1].level, LogLevel::Warn);
    let LogDestination::Syslog(syslog) = &config.core.error_logs[1].destination else {
        panic!("expected syslog error log");
    };
    assert_eq!(
        syslog.server,
        SyslogServer::Inet("127.0.0.1:5514".parse().unwrap())
    );
    assert_eq!(syslog.facility, 16);
    assert_eq!(syslog.tag, "starling");
    assert!(syslog.nohostname);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn uses_default_error_log_and_rejects_invalid_error_log_targets() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, "events {}\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert_eq!(config.core.error_logs.len(), 1);
    assert_eq!(config.core.error_logs[0].level, LogLevel::Error);
    assert!(matches!(
        &config.core.error_logs[0].destination,
        LogDestination::File(path) if path == &prefix.join("logs/error.log")
    ));

    for (directive, expected) in [
        (
            "error_log syslog:facility=local0;",
            "no syslog server specified",
        ),
        (
            "error_log syslog:server=127.0.0.1:5514 invalid;",
            "invalid log level",
        ),
        ("error_log memory:1m;", "built without debug support"),
    ] {
        fs::write(&config_path, format!("events {{}}\n{directive}\n")).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {directive}: {}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_proxy_pass_uri_in_non_prefix_locations() {
    let cases = [
        (
            "http { server { location ~ /api/ { proxy_pass http://127.0.0.1/; } } }",
            "regular expression",
        ),
        (
            "http { server { location @backend { proxy_pass http://127.0.0.1/; } } }",
            "named location",
        ),
        (
            "http { server { location / { if ($arg_debug) { proxy_pass http://127.0.0.1/; } } } }",
            "inside \"if\" statement",
        ),
        (
            "http { server { location / { limit_except GET { proxy_pass http://127.0.0.1/; } } } }",
            "inside \"limit_except\" block",
        ),
    ];
    for (source, expected) in cases {
        let prefix = temp_prefix();
        fs::create_dir_all(prefix.join("conf")).unwrap();
        let config_path = prefix.join("conf/nginx.conf");
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
