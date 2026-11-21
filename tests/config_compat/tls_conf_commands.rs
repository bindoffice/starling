use super::*;

#[test]
fn rejects_proxy_ssl_conf_command_with_an_actionable_error() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for source in [
        r#"
            events {}
            http {
                proxy_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256;
            }
        "#,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18130;
                    location / {
                        proxy_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256;
                        proxy_pass https://127.0.0.1:19443;
                    }
                }
            }
        "#,
        r#"
            events {}
            stream {
                proxy_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256;
            }
        "#,
        r#"
            events {}
            stream {
                server {
                    listen 127.0.0.1:18131;
                    proxy_ssl on;
                    proxy_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256;
                    proxy_pass 127.0.0.1:19443;
                }
            }
        "#,
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error
                .message
                .contains("not supported by the rustls TLS backend"),
            "{error}"
        );
        assert!(error.message.contains("proxy_ssl_protocols"), "{error}");
        assert!(error.message.contains("proxy_ssl_ciphers"), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn validates_proxy_ssl_conf_command_argument_count_before_rejecting_it() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { proxy_ssl_conf_command Ciphersuites; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("invalid number of arguments in \"proxy_ssl_conf_command\" directive"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn keeps_inert_compatibility_directives_accepted() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                sendfile_max_chunk 1m;
                proxy_send_lowat 12000;
                fastcgi_send_lowat 12000;
                gzip_window 32k;
                gzip_hash 4096;
                gzip_no_buffer on;
                postpone_gzipping 1k;
                ssl_certificate_cache max=16 inactive=10m valid=1h;
                proxy_ssl_certificate_cache max=16 inactive=10m valid=1h;
                uwsgi_ssl_certificate_cache off;
                proxy_buffering on;
                server {
                    listen 127.0.0.1:18132;
                    location / { return 204; }
                }
            }
            stream {
                ssl_certificate_cache max=16 inactive=10m valid=1h;
                proxy_ssl_certificate_cache max=16 inactive=10m valid=1h;
                server {
                    listen 127.0.0.1:18133;
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
    for directive in ["proxy_send_lowat", "fastcgi_send_lowat"] {
        assert!(
            config.warnings.iter().any(|warning| {
                warning.message.contains(&format!("\"{directive}\""))
                    && warning.message.contains("not applied")
            }),
            "missing ignored warning for {directive}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn validates_inert_compatibility_directive_arguments() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, directive) in [
        (
            "events {} http { proxy_send_lowat nope; }",
            "proxy_send_lowat",
        ),
        (
            "events {} http { fastcgi_send_lowat 12kb; }",
            "fastcgi_send_lowat",
        ),
        ("events {} http { gzip_window 3k; }", "gzip_window"),
        ("events {} http { gzip_hash 256k; }", "gzip_hash"),
        (
            "events {} http { gzip_no_buffer enabled; }",
            "gzip_no_buffer",
        ),
        (
            "events {} http { postpone_gzipping later; }",
            "postpone_gzipping",
        ),
        (
            "events {} http { ssl_certificate_cache inactive=10m; }",
            "ssl_certificate_cache",
        ),
        (
            "events {} http { proxy_ssl_certificate_cache max=0; }",
            "proxy_ssl_certificate_cache",
        ),
        (
            "events {} http { uwsgi_ssl_certificate_cache active=10m; }",
            "uwsgi_ssl_certificate_cache",
        ),
        (
            "events {} stream { ssl_certificate_cache inactive=10m; }",
            "ssl_certificate_cache",
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(directive), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}
