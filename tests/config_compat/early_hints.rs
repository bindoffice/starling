use super::*;

#[test]
fn compiles_early_hints_predicates_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                early_hints $http_link;
                early_hints $arg_preload;
                server {
                    listen 127.0.0.1:18461;
                    location /inherited {}
                    location /override/ {
                        early_hints $cookie_hint;
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

    assert_eq!(
        server.context.early_hints.predicates,
        ["$http_link", "$arg_preload"]
    );
    assert_eq!(
        server.locations[0].context.early_hints.predicates,
        ["$http_link", "$arg_preload"]
    );
    assert_eq!(
        server.locations[1].context.early_hints.predicates,
        ["$cookie_hint"]
    );

    fs::write(&config_path, "events {} http { early_hints; }\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(
        error
            .message
            .contains("invalid number of arguments in \"early_hints\" directive"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
