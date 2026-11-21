use super::*;

#[test]
fn compiles_proxy_headers_hash_sizes_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_headers_hash_max_size 1024;
                proxy_headers_hash_bucket_size 128;
                server {
                    listen 127.0.0.1:18522;
                    location /inherited {
                        proxy_pass http://127.0.0.1:8080;
                    }
                    location /override {
                        proxy_headers_hash_max_size 2048;
                        proxy_headers_hash_bucket_size 256;
                        proxy_pass http://127.0.0.1:8080;
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
    for context in [&server.context, &server.locations[0].context] {
        assert_eq!(context.proxy_headers_hash_max_size, 1024);
        assert_eq!(context.proxy_headers_hash_bucket_size, 128);
    }
    let overridden = &server.locations[1].context;
    assert_eq!(overridden.proxy_headers_hash_max_size, 2048);
    assert_eq!(overridden.proxy_headers_hash_bucket_size, 256);

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn uses_proxy_headers_hash_defaults_and_rejects_invalid_values() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18523; } }",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(context.proxy_headers_hash_max_size, 512);
    assert_eq!(context.proxy_headers_hash_bucket_size, 64);

    for (source, expected) in [
        (
            "events {} http { proxy_headers_hash_max_size nope; }",
            "invalid number",
        ),
        (
            "events {} http { proxy_headers_hash_bucket_size nope; }",
            "invalid number",
        ),
        (
            "events {} http { proxy_headers_hash_max_size 512; \
             proxy_headers_hash_max_size 1024; }",
            "is duplicate",
        ),
        (
            "events {} http { proxy_headers_hash_bucket_size 64; \
             proxy_headers_hash_bucket_size 128; }",
            "is duplicate",
        ),
        (
            "events {} http { proxy_headers_hash_bucket_size; }",
            "invalid number of arguments",
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
