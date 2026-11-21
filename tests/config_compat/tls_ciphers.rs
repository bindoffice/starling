use super::*;

#[test]
fn compiles_proxy_ssl_upstream_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        prefix.join("passwords.txt"),
        "wrong-password\r\n\r\nclient-secret\r\n",
    )
    .unwrap();
    fs::write(prefix.join("server-passwords.txt"), "server-secret\n").unwrap();
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_password_file server-passwords.txt;
                proxy_ssl_crl http.crl;
                proxy_ssl_certificate http-client.crt;
                proxy_ssl_certificate_key http-client.key;
                proxy_ssl_password_file passwords.txt;
                server {
                    listen 127.0.0.1:18125;
                    location / {
                        proxy_pass https://backend.test/;
                        proxy_ssl_server_name on;
                        proxy_ssl_name localhost;
                        proxy_ssl_session_reuse off;
                        proxy_ssl_verify on;
                        proxy_ssl_trusted_certificate ca.pem;
                        proxy_ssl_verify_depth 3;
                        proxy_ssl_crl client.crl;
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
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    assert!(context.proxy_ssl_server_name);
    assert_eq!(context.proxy_ssl_name.as_deref(), Some("localhost"));
    assert!(!context.proxy_ssl_session_reuse);
    assert!(context.proxy_ssl_verify);
    assert_eq!(
        context.proxy_ssl_trusted_certificate.as_deref(),
        Some(prefix.join("ca.pem").as_path())
    );
    assert_eq!(context.proxy_ssl_verify_depth, 3);
    assert_eq!(context.ssl_passwords, vec![b"server-secret".to_vec()]);
    assert_eq!(
        context.proxy_ssl_passwords,
        vec![b"wrong-password".to_vec(), b"client-secret".to_vec()]
    );
    assert_eq!(
        context.proxy_ssl_crl.as_deref(),
        Some(prefix.join("client.crl").as_path())
    );
    assert_eq!(
        context.proxy_ssl_certificate.as_deref(),
        Some(prefix.join("http-client.crt").as_path())
    );
    assert_eq!(
        context.proxy_ssl_certificate_key.as_deref(),
        Some(prefix.join("http-client.key").as_path())
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_ssl_prefer_server_ciphers_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_prefer_server_ciphers on;
                server {
                    listen 127.0.0.1:18457 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
                server {
                    listen 127.0.0.1:18458 ssl;
                    ssl_prefer_server_ciphers off;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
            }
            stream {
                ssl_prefer_server_ciphers on;
                server {
                    listen 127.0.0.1:18459 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    proxy_pass 127.0.0.1:19000;
                }
                server {
                    listen 127.0.0.1:18460 ssl;
                    ssl_prefer_server_ciphers off;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    proxy_pass 127.0.0.1:19000;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let http = config.http.as_ref().unwrap();
    assert!(http.servers[0].context.ssl_prefer_server_ciphers);
    assert!(!http.servers[1].context.ssl_prefer_server_ciphers);
    let stream = config.stream.as_ref().unwrap();
    assert!(stream.servers[0].ssl_prefer_server_ciphers);
    assert!(!stream.servers[1].ssl_prefer_server_ciphers);

    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18461 ssl; location / { ssl_prefer_server_ciphers on; } } }",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("not allowed here"), "{error}");
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_ssl_ciphers_with_scope_inheritance_and_openssl_expressions() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_ciphers HIGH:!aNULL:!MD5;
                proxy_ssl_ciphers ECDHE+AESGCM:!ECDHE-RSA-AES256-GCM-SHA384;
                server {
                    listen 127.0.0.1:18462 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    location / { proxy_pass https://127.0.0.1:19443; }
                }
                server {
                    listen 127.0.0.1:18463 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256;
                    location / {
                        proxy_ssl_ciphers DEFAULT:!RSA;
                        proxy_pass https://127.0.0.1:19443;
                    }
                }
            }
            stream {
                ssl_ciphers DEFAULT:!ECDSA;
                proxy_ssl_ciphers CHACHA20;
                server {
                    listen 127.0.0.1:18464 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    proxy_pass 127.0.0.1:19443;
                    proxy_ssl on;
                }
                server {
                    listen 127.0.0.1:18465 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    ssl_ciphers AES128+AESGCM;
                    proxy_pass 127.0.0.1:19443;
                    proxy_ssl on;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let http = config.http.as_ref().unwrap();
    assert_eq!(
        http.servers[0].context.ssl_ciphers.suites,
        TlsCipherSuite::TLS12_DEFAULT
    );
    assert_eq!(
        http.servers[0].context.proxy_ssl_ciphers.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );
    assert_eq!(
        http.servers[1].context.ssl_ciphers.suites,
        vec![TlsCipherSuite::EcdheRsaAes128GcmSha256]
    );
    assert_eq!(
        http.servers[1].locations[0]
            .context
            .proxy_ssl_ciphers
            .suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
        ]
    );

    let stream = config.stream.as_ref().unwrap();
    assert_eq!(
        stream.servers[0].ssl_ciphers.suites,
        vec![
            TlsCipherSuite::EcdheRsaAes256GcmSha384,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaChaCha20Poly1305Sha256,
        ]
    );
    assert_eq!(
        stream.servers[0].proxy_ssl_ciphers.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
            TlsCipherSuite::EcdheRsaChaCha20Poly1305Sha256,
        ]
    );
    assert_eq!(
        stream.servers[1].ssl_ciphers.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn accepts_legacy_cipher_names_as_a_warned_supported_subset() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18467 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    ssl_ciphers ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384:DHE-RSA-AES128-GCM-SHA256:ECDHE-RSA-AES256-SHA384:ECDHE-RSA-AES128-SHA256:!aNULL:!eNULL:!EXPORT:!DSS:!DES:!RC4:!3DES:!MD5:!PSK:!KRB5:!SRP:!CAMELLIA:!SEED;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let ciphers = &config.http.as_ref().unwrap().servers[0].context.ssl_ciphers;
    assert_eq!(
        ciphers.suites,
        vec![
            TlsCipherSuite::EcdheEcdsaAes256GcmSha384,
            TlsCipherSuite::EcdheRsaAes256GcmSha384,
            TlsCipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
            TlsCipherSuite::EcdheRsaChaCha20Poly1305Sha256,
            TlsCipherSuite::EcdheEcdsaAes128GcmSha256,
            TlsCipherSuite::EcdheRsaAes128GcmSha256,
        ]
    );
    for cipher in [
        "DHE-RSA-AES256-GCM-SHA384",
        "DHE-RSA-AES128-GCM-SHA256",
        "ECDHE-RSA-AES256-SHA384",
        "ECDHE-RSA-AES128-SHA256",
    ] {
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.message.contains(cipher)),
            "missing warning for {cipher}: {:?}",
            config.warnings
        );
    }
    assert!(
        !config
            .warnings
            .iter()
            .any(|warning| warning.message.contains("KRB5"))
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_ssl_cipher_lists() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { ssl_ciphers NOT_A_CIPHER; }",
            "unknown cipher",
        ),
        (
            "events {} http { ssl_ciphers HIGH:!aNULL::!MD5; }",
            "empty cipher-list component",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18466 ssl; location / { ssl_ciphers HIGH; } } }",
            "not allowed here",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18466; location / { proxy_pass https://127.0.0.1:19443; proxy_ssl_ciphers DEFAULT:!ALL; } } }",
            "no supported TLSv1.2 cipher suites",
        ),
        (
            "events {} stream { proxy_ssl_ciphers BAD_CIPHER; }",
            "unknown cipher",
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
