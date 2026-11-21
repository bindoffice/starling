use super::*;

#[test]
fn compiles_uwsgi_tls_configuration_and_scheme() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    fs::write(prefix.join("passwords.txt"), "secret\nsecond\n").unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                uwsgi_ssl_server_name on;
                uwsgi_ssl_name $host;
                uwsgi_ssl_session_reuse off;
                uwsgi_ssl_verify on;
                uwsgi_ssl_trusted_certificate ca.pem;
                uwsgi_ssl_verify_depth 3;
                uwsgi_ssl_crl client.crl;
                uwsgi_ssl_certificate client.crt;
                uwsgi_ssl_certificate_key client.key;
                uwsgi_ssl_password_file passwords.txt;
                uwsgi_ssl_protocols TLSv1.2;
                uwsgi_ssl_ciphers DEFAULT:!RSA;
                server {
                    listen 127.0.0.1:18510;
                    location /secure {
                        uwsgi_pass suwsgi://127.0.0.1:9001;
                    }
                    location /plain {
                        uwsgi_pass uwsgi://127.0.0.1:9001;
                    }
                    location /override {
                        uwsgi_ssl_server_name off;
                        uwsgi_ssl_name localhost;
                        uwsgi_ssl_session_reuse on;
                        uwsgi_ssl_verify off;
                        uwsgi_ssl_verify_depth 1;
                        uwsgi_ssl_protocols TLSv1.3;
                        uwsgi_ssl_ciphers CHACHA20;
                        uwsgi_pass $uwsgi_backend;
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
    let context = &server.context;
    assert!(context.uwsgi_ssl_server_name);
    assert_eq!(context.uwsgi_ssl_name.as_deref(), Some("$host"));
    assert!(!context.uwsgi_ssl_session_reuse);
    assert!(context.uwsgi_ssl_verify);
    assert_eq!(
        context.uwsgi_ssl_trusted_certificate,
        Some(prefix.join("ca.pem"))
    );
    assert_eq!(context.uwsgi_ssl_verify_depth, 3);
    assert_eq!(context.uwsgi_ssl_crl, Some(prefix.join("client.crl")));
    assert_eq!(
        context.uwsgi_ssl_certificate,
        Some(prefix.join("client.crt"))
    );
    assert_eq!(
        context.uwsgi_ssl_certificate_key,
        Some(prefix.join("client.key"))
    );
    assert_eq!(
        context.uwsgi_ssl_passwords,
        vec![b"secret".to_vec(), b"second".to_vec()]
    );
    assert!(context.uwsgi_ssl_protocols.tls12);
    assert!(!context.uwsgi_ssl_protocols.tls13);
    assert!(!context.uwsgi_ssl_ciphers.suites.is_empty());

    let secure = &server.locations[0].context;
    assert_eq!(
        secure.uwsgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("suwsgi://127.0.0.1:9001")
    );
    assert!(secure.uwsgi_pass.as_ref().unwrap().tls);

    let plain = &server.locations[1].context;
    assert_eq!(
        plain.uwsgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("uwsgi://127.0.0.1:9001")
    );
    assert!(!plain.uwsgi_pass.as_ref().unwrap().tls);

    let override_ = &server.locations[2].context;
    assert!(!override_.uwsgi_ssl_server_name);
    assert_eq!(override_.uwsgi_ssl_name.as_deref(), Some("localhost"));
    assert!(override_.uwsgi_ssl_session_reuse);
    assert!(!override_.uwsgi_ssl_verify);
    assert_eq!(override_.uwsgi_ssl_verify_depth, 1);
    assert!(!override_.uwsgi_ssl_protocols.tls12);
    assert!(override_.uwsgi_ssl_protocols.tls13);
    assert!(!override_.uwsgi_ssl_ciphers.suites.is_empty());
    assert!(!override_.uwsgi_pass.as_ref().unwrap().tls);

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_uwsgi_tls_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { uwsgi_ssl_server_name maybe; }",
            "invalid value",
        ),
        (
            "events {} http { uwsgi_ssl_verify_depth nope; }",
            "invalid value",
        ),
        (
            "events {} http { uwsgi_ssl_protocols TLSv1.1; }",
            "selects no protocols supported",
        ),
        (
            "events {} http { uwsgi_ssl_ciphers NOT_A_CIPHER; }",
            "invalid cipher list",
        ),
        (
            "events {} http { uwsgi_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256; }",
            "uwsgi_ssl_conf_command",
        ),
        (
            "events {} http { uwsgi_ssl_certificate; }",
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
