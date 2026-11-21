use super::*;

#[test]
fn compiles_proxy_cookie_flags_and_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_cookie_flags $http_x_cookie secure;
                server {
                    listen 127.0.0.1:18550;
                    proxy_cookie_flags session samesite=lax $flag;
                    location /inherited {}
                    location /rules {
                        proxy_cookie_flags ~^api_ secure;
                        proxy_cookie_flags ~_session$ httponly samesite=strict;
                    }
                    location /off {
                        proxy_cookie_flags off;
                    }
                }
                server {
                    listen 127.0.0.1:18551;
                    location /http-inherited {}
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let servers = &config.http.as_ref().unwrap().servers;

    let inherited = &servers[1].locations[0].context.proxy_cookie_flags;
    assert_eq!(inherited.len(), 1);
    assert_eq!(inherited[0].cookie, "$http_x_cookie");
    assert_eq!(inherited[0].flags, ["secure"]);
    assert!(inherited[0].regex.is_none());

    let server_rules = &servers[0].context.proxy_cookie_flags;
    assert_eq!(server_rules.len(), 1);
    assert_eq!(server_rules[0].cookie, "session");
    assert_eq!(server_rules[0].flags, ["samesite=lax", "$flag"]);
    let inherited_server_rule = &servers[0].locations[0].context.proxy_cookie_flags;
    assert_eq!(inherited_server_rule.len(), 1);
    assert_eq!(inherited_server_rule[0].cookie, server_rules[0].cookie);
    assert_eq!(inherited_server_rule[0].flags, server_rules[0].flags);

    let rules = &servers[0].locations[1].context.proxy_cookie_flags;
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].cookie, "^api_");
    assert!(rules[0].regex.as_ref().unwrap().is_match("API_session"));
    assert_eq!(rules[1].cookie, "_session$");
    assert_eq!(rules[1].flags, ["httponly", "samesite=strict"]);
    assert!(
        servers[0].locations[2]
            .context
            .proxy_cookie_flags
            .is_empty()
    );

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_proxy_cookie_flags() {
    for (directive, expected) in [
        (
            "proxy_cookie_flags;",
            "invalid number of arguments in \"proxy_cookie_flags\" directive",
        ),
        (
            "proxy_cookie_flags session;",
            "invalid parameter \"session\"",
        ),
        (
            "proxy_cookie_flags ~( secure;",
            "invalid regular expression",
        ),
        (
            "proxy_cookie_flags off; proxy_cookie_flags off;",
            "\"proxy_cookie_flags\" directive is duplicate",
        ),
        (
            "proxy_cookie_flags off; proxy_cookie_flags session secure;",
            "\"proxy_cookie_flags\" directive is duplicate",
        ),
        (
            "proxy_cookie_flags session secure; proxy_cookie_flags off;",
            "\"proxy_cookie_flags\" directive is duplicate",
        ),
    ] {
        let error = compile_error(&format!(
            "events {{}} http {{ server {{ listen 127.0.0.1:18552; {directive} }} }}"
        ));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn allows_multiple_proxy_cookie_flag_rules() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18553;
                    proxy_cookie_flags first secure;
                    proxy_cookie_flags second httponly unrelated-flag;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let rules = &config.http.as_ref().unwrap().servers[0]
        .context
        .proxy_cookie_flags;
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[1].flags, ["httponly", "unrelated-flag"]);
    let _ = fs::remove_dir_all(prefix);
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
