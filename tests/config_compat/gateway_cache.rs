use super::*;
use starling::config::{GatewayCacheConfig, ProxyCacheConfig};

#[test]
fn compiles_gateway_cache_directives_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_cache_path cache/fastcgi levels=1:2 keys_zone=fast:8m;
                uwsgi_cache_path cache/uwsgi levels=1:2 keys_zone=uw:8m;
                scgi_cache_path cache/scgi levels=1:2 keys_zone=sc:8m;

                fastcgi_cache fast;
                fastcgi_cache_key "$scheme$request_method$host$request_uri";
                fastcgi_cache_valid 200 301 10m;
                fastcgi_cache_valid 404 1m;
                fastcgi_cache_min_uses 2;
                fastcgi_cache_max_range_offset 4k;
                fastcgi_cache_methods POST;
                fastcgi_cache_bypass $http_cache_bypass;
                fastcgi_no_cache $http_pragma;
                fastcgi_cache_lock on;
                fastcgi_cache_lock_timeout 8s;
                fastcgi_cache_lock_age 3s;
                fastcgi_cache_revalidate on;
                fastcgi_cache_background_update on;
                fastcgi_cache_use_stale error timeout updating http_500;
                fastcgi_cache_use_stale http_502;

                uwsgi_cache uw;
                uwsgi_cache_key "$request_uri";
                uwsgi_cache_valid 201 5m;

                scgi_cache sc;
                scgi_cache_key "$request_uri";
                scgi_cache_valid 202 6m;

                server {
                    listen 127.0.0.1:18524;
                    location /inherited {
                        fastcgi_pass 127.0.0.1:9000;
                        uwsgi_pass 127.0.0.1:9001;
                        scgi_pass 127.0.0.1:9002;
                    }
                    location /override {
                        fastcgi_cache off;
                        fastcgi_cache_valid 404 30s;
                        fastcgi_cache_methods GET;
                        fastcgi_cache_lock off;
                        fastcgi_cache_revalidate off;
                        fastcgi_cache_background_update off;
                        fastcgi_cache_use_stale off;
                        fastcgi_pass 127.0.0.1:9000;
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
    assert!(http.cache_paths.contains_key("fast"));
    assert!(http.cache_paths.contains_key("uw"));
    assert!(http.cache_paths.contains_key("sc"));

    let inherited = &http.servers[0].locations[0].context;
    assert!(matches!(
        inherited.fastcgi_cache.cache,
        ProxyCacheConfig::Zone(ref zone) if zone == "fast"
    ));
    assert_eq!(
        inherited.fastcgi_cache.key,
        "$scheme$request_method$host$request_uri"
    );
    assert_eq!(inherited.fastcgi_cache.valid.len(), 2);
    assert_eq!(inherited.fastcgi_cache.valid[0].statuses, vec![200, 301]);
    assert_eq!(inherited.fastcgi_cache.valid[1].statuses, vec![404]);
    assert_eq!(inherited.fastcgi_cache.min_uses, 2);
    assert_eq!(inherited.fastcgi_cache.max_range_offset, Some(4 * 1024));
    assert!(inherited.fastcgi_cache.methods.allows("GET"));
    assert!(inherited.fastcgi_cache.methods.allows("HEAD"));
    assert!(inherited.fastcgi_cache.methods.allows("POST"));
    assert!(inherited.fastcgi_cache.lock.enabled);
    assert_eq!(inherited.fastcgi_cache.lock.timeout, Duration::from_secs(8));
    assert_eq!(inherited.fastcgi_cache.lock.age, Duration::from_secs(3));
    assert!(inherited.fastcgi_cache.revalidate);
    assert!(inherited.fastcgi_cache.background_update);
    assert!(inherited.fastcgi_cache.use_stale.error());
    assert!(inherited.fastcgi_cache.use_stale.timeout());
    assert!(inherited.fastcgi_cache.use_stale.updating());
    assert!(inherited.fastcgi_cache.use_stale.status(500));
    assert!(inherited.fastcgi_cache.use_stale.status(502));
    assert!(matches!(
        inherited.uwsgi_cache.cache,
        ProxyCacheConfig::Zone(ref zone) if zone == "uw"
    ));
    assert!(matches!(
        inherited.scgi_cache.cache,
        ProxyCacheConfig::Zone(ref zone) if zone == "sc"
    ));

    let overridden = &http.servers[0].locations[1].context;
    assert!(matches!(
        overridden.fastcgi_cache.cache,
        ProxyCacheConfig::Off
    ));
    assert_eq!(overridden.fastcgi_cache.valid.len(), 1);
    assert_eq!(
        overridden.fastcgi_cache.valid[0].valid,
        Duration::from_secs(30)
    );
    assert!(!overridden.fastcgi_cache.methods.allows("POST"));
    assert!(!overridden.fastcgi_cache.lock.enabled);
    assert!(!overridden.fastcgi_cache.revalidate);
    assert!(!overridden.fastcgi_cache.background_update);
    assert!(!overridden.fastcgi_cache.use_stale.error());
    assert!(!overridden.fastcgi_cache.use_stale.status(502));

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn validates_gateway_cache_defaults_zones_and_arguments() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18525; } }",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.fastcgi_cache, GatewayCacheConfig::default());
    assert_eq!(context.uwsgi_cache, GatewayCacheConfig::default());
    assert_eq!(context.scgi_cache, GatewayCacheConfig::default());

    for (source, expected) in [
        (
            "events {} http { fastcgi_cache absent; server { listen 127.0.0.1:18526; } }",
            "\"fastcgi_cache\" zone \"absent\" is unknown",
        ),
        (
            "events {} http { uwsgi_cache absent; server { listen 127.0.0.1:18527; } }",
            "\"uwsgi_cache\" zone \"absent\" is unknown",
        ),
        (
            "events {} http { scgi_cache absent; server { listen 127.0.0.1:18528; } }",
            "\"scgi_cache\" zone \"absent\" is unknown",
        ),
        (
            "events {} http { fastcgi_cache_key one; fastcgi_cache_key two; }",
            "\"fastcgi_cache_key\" directive is duplicate",
        ),
        (
            "events {} http { uwsgi_cache_methods PATCH; }",
            "invalid value \"PATCH\" in \"uwsgi_cache_methods\" directive",
        ),
        (
            "events {} http { scgi_cache_valid 700 1m; }",
            "invalid value \"700\" in \"scgi_cache_valid\" directive",
        ),
        (
            "events {} http { fastcgi_cache_path cache keys_zone=; }",
            "invalid parameter \"keys_zone=\"",
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }

    let _ = fs::remove_dir_all(prefix);
}
