use super::*;

#[test]
fn compiles_http2_directive_and_listen_option_with_and_without_tls() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                http2 on;
                server {
                    listen 127.0.0.1:18443 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    location / { return 200 "h2\n"; }
                }
                server {
                    listen 127.0.0.1:18444 ssl;
                    http2 off;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
                server {
                    listen 127.0.0.1:18445 ssl http2;
                    http2 off;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let servers = &config.http.as_ref().unwrap().servers;
    assert!(servers[0].http2);
    assert!(!servers[0].listens[0].http2);
    assert!(!servers[1].http2);
    assert!(!servers[2].http2);
    assert!(servers[2].listens[0].http2);

    for (source, server_http2, listen_http2) in [
        (
            "events {} http { server { listen 127.0.0.1:8080 http2; } }\n",
            false,
            true,
        ),
        (
            "events {} http { server { listen 127.0.0.1:8080; http2 on; } }\n",
            true,
            false,
        ),
        (
            "events {} http { http2 on; server { listen 127.0.0.1:8080; } }\n",
            true,
            false,
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
        let server = &config.http.as_ref().unwrap().servers[0];
        assert_eq!(server.http2, server_http2, "{source}");
        assert_eq!(server.listens[0].http2, listen_http2, "{source}");
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_http2_runtime_limits_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                http2_recv_buffer_size 512k;
                http2_max_concurrent_streams 64;
                http2_body_preread_size 32k;
                http2_chunk_size 4k;
                http2_max_field_size 4k;
                http2_max_header_size 64k;
                http2_max_requests 1000;
                http2_streams_index_size 64;
                server {
                    listen 127.0.0.1:18450 ssl http2;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    http2_max_concurrent_streams 32;
                    http2_body_preread_size 16k;
                    location / {
                        http2_chunk_size 2k;
                    }
                    location /inherited {}
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.http2_limits.max_concurrent_streams, 32);
    assert_eq!(server.context.http2_limits.body_preread_size, 16 * 1024);
    assert_eq!(server.context.http2_limits.max_header_list_size, 32 * 1024);
    assert_eq!(config.warnings.len(), 2);
    for (warning, directive) in config
        .warnings
        .iter()
        .zip(["http2_max_field_size", "http2_max_header_size"])
    {
        assert!(
            warning.message.contains(&format!(
                "the \"{directive}\" directive is obsolete, use the \
                 \"large_client_header_buffers\" directive instead"
            )),
            "{warning}"
        );
    }
    assert_eq!(
        server.locations[0].context.http2_limits.chunk_size,
        2 * 1024
    );
    assert_eq!(
        server.locations[1].context.http2_limits.chunk_size,
        4 * 1024
    );

    for (directive, expected) in [
        ("http2_chunk_size 0;", "the http2 chunk size cannot be zero"),
        ("http2_streams_index_size 3;", "must be a power of two"),
        (
            "http2_max_concurrent_streams 1; http2_max_concurrent_streams 2;",
            "\"http2_max_concurrent_streams\" directive is duplicate",
        ),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {directive}: {error}"
        );
    }

    fs::write(
        &config_path,
        "events {} http { server { location / { http2_recv_buffer_size 1m; } } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("\"http2_recv_buffer_size\" directive is not allowed here"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn accepts_obsolete_http2_header_directives_without_parsing_their_values() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                http2_max_field_size not-a-size;
                http2_max_header_size 64k;
                http2_max_header_size also-not-a-size;
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(config.warnings.len(), 3);
    assert!(config.warnings.iter().all(|warning| {
        warning.message.contains("directive is obsolete")
            && warning.message.contains("large_client_header_buffers")
    }));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_ssl_protocols_with_http_and_stream_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_protocols TLSv1.2;
                server {
                    listen 127.0.0.1:18446 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
                server {
                    listen 127.0.0.1:18447 ssl;
                    ssl_protocols TLSv1.3;
                    ssl_protocols tlsv1.2;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
            }
            stream {
                ssl_protocols TLSv1.3;
                server {
                    listen 127.0.0.1:18448 ssl;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    proxy_pass 127.0.0.1:19000;
                }
                server {
                    listen 127.0.0.1:18449 ssl;
                    ssl_protocols TLSv1.2;
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
    let config = compile(document, prefix.clone(), config_path).unwrap();

    let http = config.http.as_ref().unwrap();
    assert!(http.servers[0].context.ssl_protocols.tls12);
    assert!(!http.servers[0].context.ssl_protocols.tls13);
    assert!(http.servers[1].context.ssl_protocols.tls12);
    assert!(http.servers[1].context.ssl_protocols.tls13);

    let stream = config.stream.as_ref().unwrap();
    assert!(!stream.servers[0].ssl_protocols.tls12);
    assert!(stream.servers[0].ssl_protocols.tls13);
    assert!(stream.servers[1].ssl_protocols.tls12);
    assert!(!stream.servers[1].ssl_protocols.tls13);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn accepts_mixed_legacy_tls_protocols_with_visible_warnings() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18451 ssl;
                    ssl_protocols SSLv3 TLSv1 TLSv1.1 TLSv1.2 TLSv1.3;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let protocols = config.http.as_ref().unwrap().servers[0]
        .context
        .ssl_protocols;
    assert!(protocols.tls12);
    assert!(protocols.tls13);
    assert_eq!(config.warnings.len(), 3);
    for protocol in ["SSLv3", "TLSv1", "TLSv1.1"] {
        assert!(
            config
                .warnings
                .iter()
                .any(|warning| warning.message.contains(protocol)),
            "{:?}",
            config.warnings
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_or_legacy_only_ssl_protocols() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18450 ssl; ssl_protocols; } }",
            "invalid number of arguments",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18450 ssl; ssl_protocols TLSv9; } }",
            "invalid value",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18450 ssl; ssl_protocols TLSv1.1; } }",
            "selects no protocols supported",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18450 ssl; location / { ssl_protocols TLSv1.2; } } }",
            "not allowed here",
        ),
        (
            "events {} stream { server { listen 127.0.0.1:18450 ssl; ssl_protocols SSLv3; proxy_pass 127.0.0.1:19000; } }",
            "selects no protocols supported",
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
fn compiles_proxy_ssl_protocols_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_ssl_protocols TLSv1.2;
                server {
                    listen 127.0.0.1:18452;
                    location / { proxy_pass https://127.0.0.1:19443; }
                }
                server {
                    listen 127.0.0.1:18453;
                    proxy_ssl_protocols TLSv1.3;
                    location / { proxy_pass https://127.0.0.1:19443; }
                    location /tls12 {
                        proxy_ssl_protocols TLSv1.2;
                        proxy_pass https://127.0.0.1:19443;
                    }
                }
            }
            stream {
                proxy_ssl_protocols TLSv1.3;
                server {
                    listen 127.0.0.1:18454;
                    proxy_pass 127.0.0.1:19443;
                    proxy_ssl on;
                }
                server {
                    listen 127.0.0.1:18455;
                    proxy_ssl_protocols TLSv1.2;
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
    let config = compile(document, prefix.clone(), config_path).unwrap();

    let http = config.http.as_ref().unwrap();
    let inherited = &http.servers[0].locations[0].context.proxy_ssl_protocols;
    assert!(inherited.tls12);
    assert!(!inherited.tls13);
    let server_override = &http.servers[1].locations[0].context.proxy_ssl_protocols;
    assert!(!server_override.tls12);
    assert!(server_override.tls13);
    let location_override = &http.servers[1].locations[1].context.proxy_ssl_protocols;
    assert!(location_override.tls12);
    assert!(!location_override.tls13);

    let stream = config.stream.as_ref().unwrap();
    assert!(!stream.servers[0].proxy_ssl_protocols.tls12);
    assert!(stream.servers[0].proxy_ssl_protocols.tls13);
    assert!(stream.servers[1].proxy_ssl_protocols.tls12);
    assert!(!stream.servers[1].proxy_ssl_protocols.tls13);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_proxy_ssl_protocols() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18456; location / { proxy_pass https://127.0.0.1:19443; proxy_ssl_protocols TLSv9; } } }",
            "invalid value",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18456; location / { proxy_pass https://127.0.0.1:19443; proxy_ssl_protocols TLSv1.1; } } }",
            "selects no protocols supported",
        ),
        (
            "events {} stream { server { listen 127.0.0.1:18456; proxy_pass 127.0.0.1:19443; proxy_ssl on; proxy_ssl_protocols SSLv3; } }",
            "selects no protocols supported",
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
