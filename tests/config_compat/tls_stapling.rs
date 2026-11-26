use super::*;

#[test]
fn compiles_stapling_and_resolver_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                resolver 1.1.1.1 8.8.8.8:5353 [2001:db8::1] valid=10s ipv6=off;
                resolver_timeout 1500ms;
                ssl_stapling on;
                ssl_stapling_verify on;
                ssl_stapling_responder http://127.0.0.1:8080/ocsp;
                ssl_trusted_certificate trusted.pem;
                server {
                    listen 127.0.0.1:18471 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    location / {
                        resolver 127.0.0.1;
                        resolver_timeout 2s;
                        return 204;
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
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert!(context.ssl_stapling);
    assert!(context.ssl_stapling_verify);
    assert_eq!(
        context.ssl_stapling_responder.as_deref(),
        Some("http://127.0.0.1:8080/ocsp")
    );
    assert_eq!(context.resolver.servers.len(), 3);
    assert_eq!(context.resolver.valid, Some(Duration::from_secs(10)));
    assert!(!context.resolver.ipv6);
    assert_eq!(context.resolver_timeout, Duration::from_millis(1500));
    let location = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    assert_eq!(
        location.resolver.servers,
        vec!["127.0.0.1:53".parse().unwrap()]
    );
    assert_eq!(location.resolver_timeout, Duration::from_secs(2));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stapling_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18472 ssl; ssl_stapling on; ssl_stapling_verify on; } }",
            "no ssl_trusted_certificate for ssl_stapling_verify",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18472 ssl; resolver nope; } }",
            "invalid resolver address",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18472 ssl; location / { ssl_stapling on; } } }",
            "\"ssl_stapling\" directive is not allowed here",
        ),
        (
            "events {} http { resolver 1.1.1.1; resolver 8.8.8.8; }",
            "\"resolver\" directive is duplicate",
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

#[test]
fn preserves_multiple_certificate_and_key_pairs_in_order() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18473 ssl;
                    ssl_certificate rsa.crt;
                    ssl_certificate ec.crt;
                    ssl_certificate_key rsa.key;
                    ssl_certificate_key ec.key;
                }
            }
            stream {
                server {
                    listen 127.0.0.1:18474 ssl;
                    ssl_certificate rsa.crt;
                    ssl_certificate ec.crt;
                    ssl_certificate_key rsa.key;
                    ssl_certificate_key ec.key;
                    proxy_pass 127.0.0.1:19443;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let http = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(
        http.ssl_certificates,
        vec![prefix.join("rsa.crt"), prefix.join("ec.crt")]
    );
    assert_eq!(
        http.ssl_certificate_keys,
        vec![prefix.join("rsa.key"), prefix.join("ec.key")]
    );
    let stream = &config.stream.as_ref().unwrap().servers[0];
    assert_eq!(
        stream.ssl_certificates,
        vec![prefix.join("rsa.crt"), prefix.join("ec.crt")]
    );
    assert_eq!(
        stream.ssl_certificate_keys,
        vec![prefix.join("rsa.key"), prefix.join("ec.key")]
    );
    let _ = fs::remove_dir_all(prefix);
}
