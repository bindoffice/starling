use super::*;
use std::sync::Arc;

#[test]
fn compiles_open_file_cache_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                open_file_cache max=8 inactive=20s;
                open_file_cache_valid 5s;
                open_file_cache_min_uses 2;
                open_file_cache_errors on;
                open_file_cache_events on;
                server {
                    listen 127.0.0.1:18214;
                    location /inherited {}
                    location /override {
                        open_file_cache max=2;
                        open_file_cache_valid 1s;
                    }
                    location /off {
                        open_file_cache off;
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
    let inherited = &server.locations[0].context.open_file_cache;
    assert!(inherited.enabled);
    assert_eq!(inherited.max, 8);
    assert_eq!(inherited.inactive, Duration::from_secs(20));
    assert_eq!(inherited.valid, Duration::from_secs(5));
    assert_eq!(inherited.min_uses, 2);
    assert!(inherited.errors);
    assert!(inherited.events);
    assert!(Arc::ptr_eq(inherited, &server.context.open_file_cache));

    let overridden = &server.locations[1].context.open_file_cache;
    assert!(overridden.enabled);
    assert_eq!(overridden.max, 2);
    assert_eq!(overridden.valid, Duration::from_secs(1));
    assert!(!server.locations[2].context.open_file_cache.enabled);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_open_file_cache_parameters() {
    for (parameters, expected) in [
        ("", "invalid number of arguments"),
        ("max=0", "positive \"max\""),
        ("inactive=10s", "invalid \"open_file_cache\" parameter"),
        ("max=4 inactive=invalid", "invalid time value"),
        ("max=4 valid=1s", "invalid \"open_file_cache\" parameter"),
        ("off extra", "invalid \"open_file_cache\" parameter"),
    ] {
        let source = format!(
            r#"
                events {{}}
                http {{
                    server {{
                        open_file_cache {parameters};
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

    let zero_min_uses = config_error(
        r#"
            events {}
            http {
                server {
                    open_file_cache_min_uses 0;
                }
            }
        "#,
    );
    assert!(
        zero_min_uses.contains("invalid value in \"open_file_cache_min_uses\""),
        "{zero_min_uses}"
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
