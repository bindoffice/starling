use super::*;

#[test]
fn compiles_http_error_logs_with_nginx_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                error_log logs/http.log info;
                error_log logs/http-second.log notice;
                server {
                    listen 127.0.0.1:18360;
                    location /inherited { return 204; }
                    location /override {
                        error_log logs/location.log warn;
                        error_log logs/location-second.log debug;
                        return 204;
                    }
                }
                server {
                    listen 127.0.0.1:18361;
                    error_log logs/server.log error;
                    location /inherited { return 204; }
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

    assert_eq!(http.error_logs.len(), 2);
    assert_eq!(http.error_logs[0].level, LogLevel::Info);
    assert!(matches!(
        &http.error_logs[0].destination,
        LogDestination::File(path) if path == &prefix.join("logs/http.log")
    ));
    assert_eq!(http.error_logs[1].level, LogLevel::Notice);

    let inherited_server = &http.servers[0];
    assert_eq!(inherited_server.context.error_logs, http.error_logs);
    assert_eq!(
        inherited_server.locations[0].context.error_logs,
        http.error_logs
    );

    let override_location = &inherited_server.locations[1];
    assert_eq!(override_location.context.error_logs.len(), 2);
    assert_eq!(
        override_location.context.error_logs[0].level,
        LogLevel::Warn
    );
    assert!(matches!(
        &override_location.context.error_logs[0].destination,
        LogDestination::File(path) if path == &prefix.join("logs/location.log")
    ));
    assert_eq!(
        override_location.context.error_logs[1].level,
        LogLevel::Debug
    );

    let override_server = &http.servers[1];
    assert_eq!(override_server.context.error_logs.len(), 1);
    assert_eq!(override_server.context.error_logs[0].level, LogLevel::Error);
    assert_eq!(
        override_server.locations[0].context.error_logs,
        override_server.context.error_logs
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_http_error_log_levels_at_each_scope() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for source in [
        "events {} http { error_log logs/http.log nonsense; }\n",
        "events {} http { server { error_log logs/server.log nonsense; } }\n",
        "events {} http { server { location / { error_log logs/location.log nonsense; } } }\n",
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains("invalid log level"),
            "unexpected error: {}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
