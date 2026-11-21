use super::*;
use std::sync::Arc;

#[test]
fn compiles_and_inherits_open_log_file_cache() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                open_log_file_cache max=8 inactive=20s min_uses=2 valid=5s;
                server {
                    listen 127.0.0.1:18290;
                    location /inherited {}
                    location /override {
                        open_log_file_cache max=2;
                    }
                    location /off {
                        open_log_file_cache off;
                    }
                }
                server {
                    listen 127.0.0.1:18291;
                    open_log_file_cache max=1 inactive=2s;
                    location / {}
                }
            }
            stream {
                open_log_file_cache max=16 min_uses=3;
                server {
                    listen 127.0.0.1:13390;
                    return "ok";
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
    let server = &http.servers[0];
    let inherited = server.context.open_log_file_cache.as_ref().unwrap();
    assert_eq!(inherited.max, 8);
    assert_eq!(inherited.inactive, Duration::from_secs(20));
    assert_eq!(inherited.min_uses, 2);
    assert_eq!(inherited.valid, Duration::from_secs(5));
    assert!(Arc::ptr_eq(
        inherited,
        server.locations[0]
            .context
            .open_log_file_cache
            .as_ref()
            .unwrap()
    ));

    let overridden = server.locations[1]
        .context
        .open_log_file_cache
        .as_ref()
        .unwrap();
    assert_eq!(overridden.max, 2);
    assert_eq!(overridden.inactive, Duration::from_secs(10));
    assert_eq!(overridden.min_uses, 1);
    assert_eq!(overridden.valid, Duration::from_secs(60));
    assert!(server.locations[2].context.open_log_file_cache.is_none());

    let own = http.servers[1]
        .context
        .open_log_file_cache
        .as_ref()
        .unwrap();
    assert_eq!(own.max, 1);
    assert_eq!(own.inactive, Duration::from_secs(2));
    assert!(Arc::ptr_eq(
        own,
        http.servers[1].locations[0]
            .context
            .open_log_file_cache
            .as_ref()
            .unwrap()
    ));

    let stream = config.stream.as_ref().unwrap();
    let stream_cache = stream.servers[0].open_log_file_cache.as_ref().unwrap();
    assert_eq!(stream_cache.max, 16);
    assert_eq!(stream_cache.min_uses, 3);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn accepts_nginx_open_log_file_cache_parameter_order_and_duplicates() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    open_log_file_cache min_uses=3 valid=2s inactive=4s max=9;
                }
                server {
                    open_log_file_cache max=9 max=10;
                }
                server {
                    open_log_file_cache off max=3;
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
    let cache = http.servers[0]
        .context
        .open_log_file_cache
        .as_ref()
        .unwrap();
    assert_eq!(cache.max, 9);
    assert_eq!(cache.inactive, Duration::from_secs(4));
    assert_eq!(cache.min_uses, 3);
    assert_eq!(cache.valid, Duration::from_secs(2));
    assert_eq!(
        http.servers[1]
            .context
            .open_log_file_cache
            .as_ref()
            .unwrap()
            .max,
        10
    );
    assert!(http.servers[2].context.open_log_file_cache.is_none());
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_open_log_file_cache_parameters() {
    for (parameters, expected) in [
        ("", "invalid number of arguments"),
        ("max=0", "must have \"max\" parameter"),
        ("inactive=10s", "must have \"max\" parameter"),
        ("max=invalid", "invalid \"open_log_file_cache\" parameter"),
        (
            "max=4 inactive=invalid",
            "invalid \"open_log_file_cache\" parameter",
        ),
        (
            "max=4 min_uses=invalid",
            "invalid \"open_log_file_cache\" parameter",
        ),
        (
            "max=4 valid=invalid",
            "invalid \"open_log_file_cache\" parameter",
        ),
        (
            "max=4 unknown=value",
            "invalid \"open_log_file_cache\" parameter",
        ),
        (
            "max=4 inactive=1s min_uses=1 valid=1s extra",
            "invalid number of arguments",
        ),
    ] {
        let source = format!(
            r#"
                events {{}}
                http {{
                    server {{
                        open_log_file_cache {parameters};
                    }}
                }}
            "#
        );
        let error = config_error(&source);
        assert!(
            error.contains(expected),
            "unexpected error for {parameters:?}: {error}"
        );
    }

    let duplicate = config_error(
        r#"
            events {}
            http {
                server {
                    open_log_file_cache max=4;
                    open_log_file_cache max=8;
                }
            }
        "#,
    );
    assert!(
        duplicate.contains("\"open_log_file_cache\" directive is duplicate"),
        "{duplicate}"
    );

    let invalid_context = config_error(
        r#"
            events {}
            http {
                server {
                    location / {
                        limit_except GET {
                            open_log_file_cache max=4;
                        }
                    }
                }
            }
        "#,
    );
    assert!(
        invalid_context.contains("\"open_log_file_cache\" directive is not allowed here"),
        "{invalid_context}"
    );
}

fn config_error(source: &str) -> String {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .expect_err("configuration should fail")
        .to_string();
    let _ = fs::remove_dir_all(prefix);
    error
}
