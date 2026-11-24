use super::*;

#[test]
fn rejects_invalid_limit_except_configurations() {
    let cases = [
        (
            "http { server { location / { limit_except TRACE { deny all; } } } }",
            "invalid method \"TRACE\"",
        ),
        (
            "http { server { location / { limit_except GET { deny all; } limit_except POST { allow all; } } } }",
            "\"limit_except\" directive is duplicate",
        ),
        (
            "http { server { location / { limit_except GET { add_header X-Test value; } } } }",
            "\"add_header\" directive is not allowed here",
        ),
        (
            "http { server { location / { limit_except GET { add_trailer X-Test value; } } } }",
            "\"add_trailer\" directive is not allowed here",
        ),
        (
            "http { server { location / { limit_except GET { grpc_pass grpc://127.0.0.1:9000; } } } }",
            "\"grpc_pass\" directive is not allowed here",
        ),
        (
            "http { server { location / { limit_except GET { memcached_pass 127.0.0.1:11211; } } } }",
            "\"memcached_pass\" directive is not allowed here",
        ),
        (
            "http { server { location / { limit_except GET { charset utf-8; } } } }",
            "\"charset\" directive is not allowed here",
        ),
        (
            "http { server { location / { limit_except GET { post_action /after; } } } }",
            "\"post_action\" directive is not allowed here",
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
fn compiles_limit_zones_and_inherited_limits() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                limit_req_zone $binary_remote_addr zone=requests:10m rate=30r/m;
                limit_conn_zone $binary_remote_addr zone=connections:10m;
                limit_req zone=requests burst=5 delay=2;
                limit_conn connections 4;
                limit_req_status 429;
                limit_conn_status 503;
                limit_req_log_level warn;
                limit_conn_log_level notice;
                server {
                    listen 127.0.0.1:18111;
                    location /strict/ {
                        limit_req zone=requests burst=1 nodelay;
                        limit_conn connections 1;
                        limit_conn_log_level info;
                        return 204;
                    }
                    location / { return 204; }
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
    let request_zone = http.limit_req_zones.get("requests").unwrap();
    assert_eq!(request_zone.rate_millis, 500);
    assert!(http.limit_conn_zones.contains_key("connections"));
    let server = &http.servers[0];
    assert_eq!(server.context.limit_reqs[0].burst, 5);
    assert_eq!(server.context.limit_reqs[0].delay, 2);
    assert_eq!(server.context.limit_conns[0].connections, 4);
    assert_eq!(server.context.limit_req_status, 429);
    assert_eq!(server.context.limit_req_log_level, LogLevel::Warn);
    assert_eq!(server.context.limit_conn_log_level, LogLevel::Notice);
    assert_eq!(server.locations[0].context.limit_reqs[0].burst, 1);
    assert!(server.locations[0].context.limit_reqs[0].nodelay);
    assert_eq!(server.locations[0].context.limit_conns[0].connections, 1);
    assert_eq!(
        server.locations[0].context.limit_conn_log_level,
        LogLevel::Info
    );
    assert_eq!(server.locations[1].context.limit_reqs[0].burst, 5);

    let invalid = [
        (
            "events {} http { limit_req zone=missing burst=1; }\n",
            "unknown limit_req_zone",
        ),
        (
            "events {} http { limit_conn missing 1; }\n",
            "unknown limit_conn_zone",
        ),
        (
            "events {} http { limit_req_status 200; }\n",
            "invalid status",
        ),
    ];
    for (source, expected) in invalid {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error: {}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn validates_stub_status_location_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18114;
                    location = /status { stub_status; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert!(
        config.http.as_ref().unwrap().servers[0].locations[0]
            .context
            .stub_status
    );

    fs::write(
        &config_path,
        "events {} http { stub_status; server { listen 127.0.0.1:18114; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("stub_status"));
    let _ = fs::remove_dir_all(prefix);
}
