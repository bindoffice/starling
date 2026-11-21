use super::*;
use starling::config::LocationMatcher;

#[test]
fn compiles_rewrite_logging_flags_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                rewrite_log on;
                uninitialized_variable_warn off;
                server {
                    listen 127.0.0.1:18392;
                    location / {
                        rewrite_log off;
                        if ($arg_mode = on) {
                            rewrite_log on;
                            uninitialized_variable_warn on;
                        }
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
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(server.context.rewrite_log);
    assert!(!server.context.uninitialized_variable_warn);
    let location = server
        .locations
        .iter()
        .find(|location| matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/"))
        .unwrap();
    assert!(!location.context.rewrite_log);
    assert!(!location.context.uninitialized_variable_warn);
    assert!(location.context.conditionals[0].rewrite_log);
    assert!(location.context.conditionals[0].uninitialized_variable_warn);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_rewrite_logging_flags() {
    for argument in ["maybe", "on off"] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(
            &config_path,
            format!(
                "events {{}} http {{ rewrite_log {argument}; server {{ listen 127.0.0.1:18393; }} }}"
            ),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path).unwrap_err();
        assert!(
            error.message.contains("rewrite_log"),
            "unexpected error for {argument}: {}",
            error.message
        );
        let _ = fs::remove_dir_all(prefix);
    }
}

#[test]
fn matches_nginx_return_code_and_redirect_validation() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18394;
                    location /zero { return 0; }
                    location /high { return 600; }
                    location /max { return 999; }
                    location /scheme { return $scheme://example.test/path; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let locations = &config.http.as_ref().unwrap().servers[0].locations;
    assert_eq!(
        locations[0]
            .context
            .return_directive
            .as_ref()
            .unwrap()
            .status,
        0
    );
    assert_eq!(
        locations[1]
            .context
            .return_directive
            .as_ref()
            .unwrap()
            .status,
        600
    );
    assert_eq!(
        locations[2]
            .context
            .return_directive
            .as_ref()
            .unwrap()
            .status,
        999
    );
    assert_eq!(
        locations[3]
            .context
            .return_directive
            .as_ref()
            .unwrap()
            .status,
        302
    );

    for (directive, expected) in [
        ("return 1000;", "invalid return code \"1000\""),
        ("return /relative;", "invalid return code \"/relative\""),
    ] {
        fs::write(
            &config_path,
            format!(
                "events {{}} http {{ server {{ listen 127.0.0.1:18394; location / {{ {directive} }} }} }}"
            ),
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
fn rejects_empty_rewrite_replacement() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18395; location / { rewrite ^ \"\"; } } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("empty replacement"), "{error}");
    let _ = fs::remove_dir_all(prefix);
}
