use super::*;

#[test]
fn compiles_proxy_cache_paths_scope_and_validity_rules() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_cache_path cache levels=1:2 keys_zone=shared:8m
                    inactive=15m max_size=1g min_free=64m use_temp_path=off;
                proxy_cache shared;
                proxy_cache_key "$scheme:$request_uri";
                proxy_cache_valid 200 301 10m;
                proxy_cache_valid 404 1m;
                proxy_cache_valid any 5m;
                proxy_cache_min_uses 2;
                proxy_cache_max_range_offset 4k;
                proxy_cache_methods POST;
                proxy_cache_convert_head off;
                proxy_cache_bypass $http_cache_bypass;
                proxy_no_cache $http_cache_control;
                proxy_cache_lock on;
                proxy_cache_lock_timeout 8s;
                proxy_cache_lock_age 3s;
                proxy_cache_revalidate on;
                proxy_cache_background_update on;
                proxy_cache_use_stale error timeout updating http_500 http_504;
                proxy_cache_use_stale http_502;
                server {
                    listen 127.0.0.1:18610;
                    location / {
                        proxy_pass http://127.0.0.1:19000;
                        proxy_cache off;
                        proxy_cache_valid 404 30s;
                        proxy_cache_max_range_offset 0;
                        proxy_cache_methods GET;
                        proxy_cache_convert_head on;
                        proxy_cache_lock off;
                        proxy_cache_revalidate off;
                        proxy_cache_background_update off;
                        proxy_cache_use_stale off;
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
    let http = config.http.as_ref().unwrap();
    let zone = http.cache_paths.get("shared").unwrap();
    assert_eq!(zone.path, prefix.join("cache"));
    assert_eq!(zone.levels, [1, 2, 0]);
    assert_eq!(zone.keys_zone_size, 8 * 1024 * 1024);
    assert_eq!(zone.inactive, Duration::from_secs(15 * 60));
    assert_eq!(zone.max_size, Some(1024 * 1024 * 1024));
    assert_eq!(zone.min_free, Some(64 * 1024 * 1024));
    assert!(!zone.use_temp_path);

    let inherited = &http.servers[0].context;
    assert!(matches!(
        inherited.proxy_cache,
        starling::config::ProxyCacheConfig::Zone(ref name) if name == "shared"
    ));
    assert_eq!(inherited.proxy_cache_key, "$scheme:$request_uri");
    assert_eq!(inherited.proxy_cache_min_uses, 2);
    assert_eq!(inherited.proxy_cache_max_range_offset, Some(4 * 1024));
    assert!(inherited.proxy_cache_methods.allows("GET"));
    assert!(inherited.proxy_cache_methods.allows("HEAD"));
    assert!(inherited.proxy_cache_methods.allows("POST"));
    assert!(!inherited.proxy_cache_methods.allows("PUT"));
    assert!(!inherited.proxy_cache_convert_head);
    assert_eq!(inherited.proxy_cache_valid.len(), 3);
    assert_eq!(inherited.proxy_cache_valid[0].statuses, vec![200, 301]);
    assert_eq!(inherited.proxy_cache_valid[1].statuses, vec![404]);
    assert_eq!(inherited.proxy_cache_valid[2].statuses, vec![0]);
    assert!(inherited.proxy_cache_lock.enabled);
    assert_eq!(inherited.proxy_cache_lock.timeout, Duration::from_secs(8));
    assert_eq!(inherited.proxy_cache_lock.age, Duration::from_secs(3));
    assert!(inherited.proxy_cache_revalidate);
    assert!(inherited.proxy_cache_background_update);
    assert!(inherited.proxy_cache_use_stale.error());
    assert!(inherited.proxy_cache_use_stale.timeout());
    assert!(inherited.proxy_cache_use_stale.updating());
    assert!(inherited.proxy_cache_use_stale.status(500));
    assert!(inherited.proxy_cache_use_stale.status(502));
    assert!(inherited.proxy_cache_use_stale.status(504));
    assert!(!inherited.proxy_cache_use_stale.status(503));

    let location = &http.servers[0].locations[0].context;
    assert!(matches!(
        location.proxy_cache,
        starling::config::ProxyCacheConfig::Off
    ));
    assert_eq!(location.proxy_cache_valid.len(), 1);
    assert_eq!(location.proxy_cache_valid[0].statuses, vec![404]);
    assert_eq!(location.proxy_cache_valid[0].valid, Duration::from_secs(30));
    assert_eq!(location.proxy_cache_max_range_offset, Some(0));
    assert!(location.proxy_cache_methods.allows("GET"));
    assert!(location.proxy_cache_methods.allows("HEAD"));
    assert!(!location.proxy_cache_methods.allows("POST"));
    assert!(location.proxy_cache_convert_head);
    assert!(!location.proxy_cache_lock.enabled);
    assert_eq!(location.proxy_cache_lock.timeout, Duration::from_secs(8));
    assert_eq!(location.proxy_cache_lock.age, Duration::from_secs(3));
    assert!(!location.proxy_cache_revalidate);
    assert!(!location.proxy_cache_background_update);
    assert!(!location.proxy_cache_use_stale.error());
    assert!(!location.proxy_cache_use_stale.timeout());
    assert!(!location.proxy_cache_use_stale.updating());
    assert!(!location.proxy_cache_use_stale.status(500));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_missing_or_duplicate_proxy_cache_zones() {
    let missing = compile_error(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18611;
                    location / {
                        proxy_pass http://127.0.0.1:19000;
                        proxy_cache absent;
                    }
                }
            }
        "#,
    );
    assert!(
        missing.contains("cache zone \"absent\" is not defined"),
        "{missing}"
    );

    let duplicate = compile_error(
        r#"
            events {}
            http {
                proxy_cache_path cache-a keys_zone=shared:1m;
                proxy_cache_path cache-b keys_zone=shared:1m;
            }
        "#,
    );
    assert!(duplicate.contains("duplicate cache zone"), "{duplicate}");
}

#[test]
fn rejects_invalid_proxy_cache_path_parameters() {
    for (directive, expected) in [
        (
            "proxy_cache_path cache keys_zone=shared:0;",
            "invalid parameter \"keys_zone=shared:0\"",
        ),
        (
            "proxy_cache_path cache keys_zone=shared:4k;",
            "zone \"keys_zone=shared:4k\" is too small",
        ),
        (
            "proxy_cache_path cache levels=1:2:3:4 keys_zone=shared:1m;",
            "invalid value \"1:2:3:4\"",
        ),
        (
            "proxy_cache_path cache keys_zone=shared:1m unknown=on;",
            "invalid parameter \"unknown=on\"",
        ),
        (
            "proxy_cache_path cache keys_zone=shared:1m purge=on;",
            "invalid parameter \"purge=on\"",
        ),
        (
            "proxy_cache_path cache;",
            "invalid number of arguments in \"proxy_cache_path\" directive",
        ),
    ] {
        let error = compile_error(&format!("events {{}} http {{ {directive} }}"));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn rejects_invalid_proxy_cache_methods_and_head_conversion() {
    for (directive, expected) in [
        (
            "proxy_cache_methods GET PUT;",
            "invalid value \"PUT\" in \"proxy_cache_methods\" directive",
        ),
        (
            "proxy_cache_methods;",
            "invalid number of arguments in \"proxy_cache_methods\" directive",
        ),
        (
            "proxy_cache_convert_head maybe;",
            "invalid value \"maybe\" in \"proxy_cache_convert_head\" directive",
        ),
        (
            "proxy_cache_convert_head on; proxy_cache_convert_head off;",
            "\"proxy_cache_convert_head\" directive is duplicate",
        ),
    ] {
        let error = compile_error(&format!("events {{}} http {{ {directive} }}"));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn rejects_invalid_proxy_cache_lock_directives() {
    for (directive, expected) in [
        (
            "proxy_cache_lock maybe;",
            "invalid value \"maybe\" in \"proxy_cache_lock\" directive",
        ),
        (
            "proxy_cache_lock_timeout nope;",
            "invalid time value \"nope\"",
        ),
        ("proxy_cache_lock_age nope;", "invalid time value \"nope\""),
        (
            "proxy_cache_lock on; proxy_cache_lock off;",
            "\"proxy_cache_lock\" directive is duplicate",
        ),
        (
            "proxy_cache_revalidate maybe;",
            "invalid value \"maybe\" in \"proxy_cache_revalidate\" directive",
        ),
        (
            "proxy_cache_revalidate on; proxy_cache_revalidate off;",
            "\"proxy_cache_revalidate\" directive is duplicate",
        ),
        (
            "proxy_cache_background_update maybe;",
            "invalid value \"maybe\" in \"proxy_cache_background_update\" directive",
        ),
        (
            "proxy_cache_background_update on; proxy_cache_background_update off;",
            "\"proxy_cache_background_update\" directive is duplicate",
        ),
        (
            "proxy_cache_use_stale maybe;",
            "invalid value \"maybe\" in \"proxy_cache_use_stale\" directive",
        ),
        (
            "proxy_cache_use_stale;",
            "invalid number of arguments in \"proxy_cache_use_stale\" directive",
        ),
    ] {
        let error = compile_error(&format!("events {{}} http {{ {directive} }}"));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn rejects_invalid_proxy_cache_max_range_offsets() {
    for (directive, expected) in [
        (
            "proxy_cache_max_range_offset nope;",
            "invalid size \"nope\"",
        ),
        ("proxy_cache_max_range_offset -1;", "invalid size \"-1\""),
        (
            "proxy_cache_max_range_offset 9223372036854775808;",
            "invalid value \"9223372036854775808\" in \"proxy_cache_max_range_offset\" directive",
        ),
        (
            "proxy_cache_max_range_offset 1; proxy_cache_max_range_offset 2;",
            "\"proxy_cache_max_range_offset\" directive is duplicate",
        ),
        (
            "proxy_cache_max_range_offset;",
            "invalid number of arguments in \"proxy_cache_max_range_offset\" directive",
        ),
    ] {
        let error = compile_error(&format!("events {{}} http {{ {directive} }}"));
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
