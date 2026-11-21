use super::*;

#[test]
fn compiles_post_action_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18471;
                    post_action /after-original;
                    location /inherited {}
                    location /override/ {
                        post_action /after-location;
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
        server.context.post_action.as_deref(),
        Some("/after-original")
    );
    assert_eq!(
        server.locations[0].context.post_action.as_deref(),
        Some("/after-original")
    );
    assert_eq!(
        server.locations[1].context.post_action.as_deref(),
        Some("/after-location")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn post_action_accepts_http_scope_and_named_locations() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                post_action @http-after;
                server {
                    listen 127.0.0.1:18472;
                    location / { return 204; }
                    location @http-after { return 204; }
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
    assert_eq!(server.context.post_action.as_deref(), Some("@http-after"));
    assert_eq!(
        server.locations[0].context.post_action.as_deref(),
        Some("@http-after")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_or_duplicate_post_action_directives() {
    for (directive, expected) in [
        (
            "post_action;",
            "invalid number of arguments in \"post_action\" directive",
        ),
        (
            "post_action /one /two;",
            "invalid number of arguments in \"post_action\" directive",
        ),
        (
            "post_action /one { }",
            "directive \"post_action\" is not terminated",
        ),
        (
            "post_action /one; post_action /two;",
            "\"post_action\" directive is duplicate",
        ),
    ] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(
            &config_path,
            format!("events {{}} http {{ server {{ listen 127.0.0.1:18473; {directive} }} }}"),
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
