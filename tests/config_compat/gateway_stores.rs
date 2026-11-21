use super::*;

#[test]
fn compiles_gateway_store_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_store /http-fastcgi/$uri;
                fastcgi_store_access user:r group:r;
                uwsgi_store /http-uwsgi/$uri;
                uwsgi_store_access user:rw all:r;
                scgi_store on;
                scgi_store_access user:r;
                server {
                    listen 127.0.0.1:18575;
                    fastcgi_store on;
                    uwsgi_store relative-uwsgi/$host$uri;
                    scgi_store off;
                    location /inherited {}
                    location /override {
                        fastcgi_store relative-fastcgi/$uri;
                        fastcgi_store_access user:r group:rw all:r;
                        uwsgi_store on;
                        uwsgi_store_access user:r group:r;
                        scgi_store /override-scgi/$uri;
                        scgi_store_access group:rw all:r;
                    }
                }
                server {
                    listen 127.0.0.1:18576;
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
    assert_eq!(first.context.fastcgi_store, StoreConfig::On);
    assert_eq!(first.context.fastcgi_store_access, 0o440);
    assert_eq!(
        first.context.uwsgi_store,
        StoreConfig::Path("relative-uwsgi/$host$uri".into())
    );
    assert_eq!(first.context.uwsgi_store_access, 0o604);
    assert_eq!(first.context.scgi_store, StoreConfig::Off);
    assert_eq!(first.context.scgi_store_access, 0o400);

    let inherited = &first.locations[0].context;
    assert_eq!(inherited.fastcgi_store, StoreConfig::On);
    assert_eq!(inherited.fastcgi_store_access, 0o440);
    assert_eq!(
        inherited.uwsgi_store,
        StoreConfig::Path("relative-uwsgi/$host$uri".into())
    );
    assert_eq!(inherited.uwsgi_store_access, 0o604);
    assert_eq!(inherited.scgi_store, StoreConfig::Off);
    assert_eq!(inherited.scgi_store_access, 0o400);

    let overridden = &first.locations[1].context;
    assert_eq!(
        overridden.fastcgi_store,
        StoreConfig::Path("relative-fastcgi/$uri".into())
    );
    assert_eq!(overridden.fastcgi_store_access, 0o464);
    assert_eq!(overridden.uwsgi_store, StoreConfig::On);
    assert_eq!(overridden.uwsgi_store_access, 0o440);
    assert_eq!(
        overridden.scgi_store,
        StoreConfig::Path("/override-scgi/$uri".into())
    );
    assert_eq!(overridden.scgi_store_access, 0o664);

    let second = &servers[1];
    assert_eq!(
        second.context.fastcgi_store,
        StoreConfig::Path("/http-fastcgi/$uri".into())
    );
    assert_eq!(second.context.fastcgi_store_access, 0o440);
    assert_eq!(
        second.context.uwsgi_store,
        StoreConfig::Path("/http-uwsgi/$uri".into())
    );
    assert_eq!(second.context.uwsgi_store_access, 0o604);
    assert_eq!(second.context.scgi_store, StoreConfig::On);
    assert_eq!(second.context.scgi_store_access, 0o400);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_gateway_stores_to_off() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18577; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.fastcgi_store, StoreConfig::Off);
    assert_eq!(context.fastcgi_store_access, 0o600);
    assert_eq!(context.uwsgi_store, StoreConfig::Off);
    assert_eq!(context.uwsgi_store_access, 0o600);
    assert_eq!(context.scgi_store, StoreConfig::Off);
    assert_eq!(context.scgi_store_access, 0o600);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_gateway_store_directives() {
    for (directive, expected) in [
        (
            "fastcgi_store;",
            "invalid number of arguments in \"fastcgi_store\" directive",
        ),
        ("uwsgi_store \"\";", "empty path"),
        (
            "scgi_store on extra;",
            "invalid number of arguments in \"scgi_store\" directive",
        ),
        (
            "fastcgi_store on; fastcgi_store off;",
            "\"fastcgi_store\" directive is duplicate",
        ),
        (
            "uwsgi_store_access;",
            "invalid number of arguments in \"uwsgi_store_access\" directive",
        ),
        (
            "scgi_store_access owner:rw;",
            "invalid value \"owner:rw\" in \"scgi_store_access\" directive",
        ),
        (
            "fastcgi_store_access user:r; fastcgi_store_access group:r;",
            "\"fastcgi_store_access\" directive is duplicate",
        ),
    ] {
        let error = compile_error(&format!(
            "events {{}} http {{ server {{ listen 127.0.0.1:18578; {directive} }} }}"
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
