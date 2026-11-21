use super::*;

#[test]
fn compiles_auth_delay_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                auth_delay 250ms;
                server {
                    listen 127.0.0.1:18210;
                    location /inherited {}
                    location /short {
                        auth_delay 20ms;
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
    assert_eq!(server.context.auth_delay, Duration::from_millis(250));
    assert_eq!(
        server.locations[0].context.auth_delay,
        Duration::from_millis(250)
    );
    assert_eq!(
        server.locations[1].context.auth_delay,
        Duration::from_millis(20)
    );

    fs::write(
        &config_path,
        "events {} http { auth_delay 1s; auth_delay 2s; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("\"auth_delay\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_limit_except_proxy_and_auth_overrides() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                auth_basic "Parent";
                auth_basic_user_file parent;
                server {
                    listen 127.0.0.1:18092;
                    location / {
                        proxy_pass http://127.0.0.1:19000;
                        limit_except GET {
                            auth_basic "Limited";
                            auth_basic_user_file conf/limited;
                            proxy_pass http://127.0.0.1:19001;
                            access_log logs/limited.log combined if=$arg_log;
                            access_log logs/limited-second.log combined;
                        }
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
    let limit_except = config.http.as_ref().unwrap().servers[0].locations[0]
        .context
        .limit_except
        .as_ref()
        .unwrap();
    let auth_file = prefix.join("conf/limited").to_string_lossy().into_owned();
    let access_log = prefix
        .join("logs/limited.log")
        .to_string_lossy()
        .into_owned();
    let access_log_path = PathBuf::from(access_log);
    assert_eq!(limit_except.auth_basic.as_deref(), Some("Limited"));
    assert_eq!(
        limit_except.auth_basic_user_file.as_deref(),
        Some(auth_file.as_str())
    );
    assert_eq!(
        limit_except.proxy_pass.as_ref().map(|proxy| proxy.port),
        Some(19001)
    );
    assert_eq!(
        limit_except
            .access_log
            .logs
            .first()
            .and_then(|config| config.destination.as_fixed_path()),
        Some(access_log_path.as_path())
    );
    assert_eq!(
        limit_except
            .access_log
            .logs
            .first()
            .and_then(|config| config.condition.as_deref()),
        Some("$arg_log")
    );
    assert_eq!(limit_except.access_log.logs.len(), 2);
    assert_eq!(
        limit_except.access_log.logs[1].destination.as_fixed_path(),
        Some(prefix.join("logs/limited-second.log").as_path())
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_auth_request_and_rejects_invalid_set_arguments() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                auth_request /auth;
                auth_request_set $auth_status $status;
                server {
                    listen 127.0.0.1:18116;
                    location = /auth {
                        internal;
                        return 204;
                    }
                    location /private/ {
                        auth_request_set $custom verified;
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
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.auth_request.as_deref(), Some("/auth"));
    assert_eq!(server.context.auth_request_sets[0].name, "auth_status");
    assert_eq!(
        server.locations[1].context.auth_request.as_deref(),
        Some("/auth")
    );
    assert_eq!(server.locations[1].context.auth_request_sets.len(), 2);

    fs::write(&config_path, "events {} http { auth_request_set $x; }\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("auth_request_set"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_and_inherits_basic_auth_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(prefix.join("conf")).unwrap();
    fs::write(prefix.join("conf/htpasswd"), "user:{PLAIN}pass\n").unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                auth_basic "Restricted";
                auth_basic_user_file conf/htpasswd;
                satisfy any;
                server {
                    listen 127.0.0.1:18109;
                    location /private/ { return 200 "private\n"; }
                    location /public/ {
                        auth_basic off;
                        return 200 "public\n";
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
    assert_eq!(server.context.auth_basic.as_deref(), Some("Restricted"));
    assert!(server.context.satisfy_any);
    assert_eq!(
        server.context.auth_basic_user_file.as_deref(),
        Some(prefix.join("conf/htpasswd").to_str().unwrap())
    );
    assert_eq!(
        server.locations[0].context.auth_basic.as_deref(),
        Some("Restricted")
    );
    assert_eq!(
        server.locations[1].context.auth_basic_user_file.as_deref(),
        Some(prefix.join("conf/htpasswd").to_str().unwrap())
    );
    assert!(server.locations[1].context.auth_basic.is_none());
    let _ = fs::remove_dir_all(prefix);
}
