use super::*;

#[test]
fn compiles_proxy_store_and_access_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_store /http-store/$uri;
                proxy_store_access user:r group:r;
                server {
                    listen 127.0.0.1:18570;
                    proxy_store on;
                    proxy_store_access user:rw all:r;
                    location /inherited {}
                    location /custom {
                        proxy_store relative-store/$host$uri;
                        proxy_store_access user:r group:rw all:r;
                    }
                    location /off {
                        proxy_store off;
                    }
                }
                server {
                    listen 127.0.0.1:18571;
                    location /inherited {}
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

    let first = &servers[0];
    assert_eq!(first.context.proxy_store, ProxyStoreConfig::On);
    assert_eq!(first.context.proxy_store_access, 0o604);
    assert_eq!(first.locations[0].context.proxy_store, ProxyStoreConfig::On);
    assert_eq!(first.locations[0].context.proxy_store_access, 0o604);
    assert_eq!(
        first.locations[1].context.proxy_store,
        ProxyStoreConfig::Path("relative-store/$host$uri".into())
    );
    assert_eq!(first.locations[1].context.proxy_store_access, 0o464);
    assert_eq!(
        first.locations[2].context.proxy_store,
        ProxyStoreConfig::Off
    );

    let inherited = &servers[1];
    assert_eq!(
        inherited.context.proxy_store,
        ProxyStoreConfig::Path("/http-store/$uri".into())
    );
    assert_eq!(inherited.context.proxy_store_access, 0o440);
    assert_eq!(
        inherited.locations[0].context.proxy_store,
        ProxyStoreConfig::Path("/http-store/$uri".into())
    );
    assert_eq!(inherited.locations[0].context.proxy_store_access, 0o440);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_proxy_store_to_off_and_access_to_0600() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18572; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.proxy_store, ProxyStoreConfig::Off);
    assert_eq!(context.proxy_store_access, 0o600);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn parses_proxy_store_access_like_nginx() {
    for (access, expected) in [
        ("user:r", 0o400),
        ("group:r", 0o640),
        ("all:r", 0o604),
        ("user:rw group:r all:r", 0o644),
        ("user:r group:rw all:r", 0o464),
    ] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(
            &config_path,
            format!(
                "events {{}} http {{ server {{ listen 127.0.0.1:18573; proxy_store_access {access}; }} }}"
            ),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let config = compile(document, prefix.clone(), config_path).unwrap();
        assert_eq!(
            config.http.as_ref().unwrap().servers[0]
                .context
                .proxy_store_access,
            expected,
            "{access}"
        );
        let _ = fs::remove_dir_all(prefix);
    }
}

#[test]
fn rejects_invalid_proxy_store_directives() {
    for (directive, expected) in [
        (
            "proxy_store;",
            "invalid number of arguments in \"proxy_store\" directive",
        ),
        (
            "proxy_store on extra;",
            "invalid number of arguments in \"proxy_store\" directive",
        ),
        ("proxy_store \"\";", "empty path"),
        (
            "proxy_store on; proxy_store off;",
            "\"proxy_store\" directive is duplicate",
        ),
        (
            "proxy_store_access;",
            "invalid number of arguments in \"proxy_store_access\" directive",
        ),
        (
            "proxy_store_access user:r group:r all:r invalid;",
            "invalid number of arguments in \"proxy_store_access\" directive",
        ),
        (
            "proxy_store_access user:rx;",
            "invalid value \"user:rx\" in \"proxy_store_access\" directive",
        ),
        (
            "proxy_store_access owner:rw;",
            "invalid value \"owner:rw\" in \"proxy_store_access\" directive",
        ),
        (
            "proxy_store_access user:r; proxy_store_access group:r;",
            "\"proxy_store_access\" directive is duplicate",
        ),
    ] {
        let error = compile_error(&format!(
            "events {{}} http {{ server {{ listen 127.0.0.1:18574; {directive} }} }}"
        ));
        assert!(error.contains(expected), "{error}");
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
