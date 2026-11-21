use super::*;

#[test]
fn compiles_grpc_endpoints_headers_and_runtime_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    fs::write(prefix.join("client-password.txt"), "secret\n").unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                grpc_connect_timeout 5s;
                grpc_send_timeout 6s;
                grpc_read_timeout 7s;
                grpc_buffer_size 8k;
                grpc_request_buffering on;
                grpc_ignore_client_abort on;
                grpc_set_header X-Scope http;
                grpc_hide_header X-From-Http;
                upstream grpc_backend {
                    server 127.0.0.1:19000;
                    keepalive 4;
                }
                server {
                    listen 127.0.0.1:18190;
                    location /direct {
                        grpc_pass grpc://127.0.0.1:9000;
                        grpc_request_buffering off;
                        grpc_ignore_client_abort off;
                        grpc_set_header Host api.example.test;
                        grpc_set_header X-Trace $request_id;
                        grpc_pass_header X-Accel-Expires;
                        grpc_ignore_headers X-Accel-Redirect Cache-Control;
                        grpc_intercept_errors on;
                        grpc_next_upstream error timeout http_500;
                        grpc_next_upstream_tries 3;
                        grpc_next_upstream_timeout 2s;
                        grpc_bind 127.0.0.2;
                        grpc_socket_keepalive on;
                        grpc_socket_rcvbuf 32k;
                        grpc_socket_sndbuf 64k;
                    }
                    location /named {
                        grpc_pass grpc_backend;
                    }
                    location /tls {
                        grpc_pass grpcs://backend.example.test;
                        grpc_ssl_server_name on;
                        grpc_ssl_name grpc.example.test;
                        grpc_ssl_session_reuse off;
                        grpc_ssl_verify on;
                        grpc_ssl_trusted_certificate tests/fixtures/ca.crt;
                        grpc_ssl_verify_depth 4;
                        grpc_ssl_crl tests/fixtures/client.crl;
                        grpc_ssl_certificate tests/fixtures/client.crt;
                        grpc_ssl_certificate_key tests/fixtures/client.key;
                        grpc_ssl_password_file client-password.txt;
                        grpc_ssl_protocols TLSv1.2;
                        grpc_ssl_ciphers ECDHE-RSA-AES128-GCM-SHA256;
                    }
                    location /dynamic {
                        grpc_pass $grpc_backend;
                        grpc_ssl_certificate_cache max=16 inactive=10m valid=1h;
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
    assert_eq!(server.context.grpc.connect_timeout, Duration::from_secs(5));
    assert_eq!(server.context.grpc.headers.len(), 1);
    assert_eq!(server.context.grpc.hide_headers, ["X-From-Http"]);
    assert!(server.context.grpc.ignore_client_abort);

    let direct = &server.locations[0].context;
    let pass = direct.grpc.pass.as_ref().unwrap();
    assert_eq!(pass.host, "127.0.0.1");
    assert_eq!(pass.port, 9000);
    assert_eq!(pass.authority, "127.0.0.1:9000");
    assert!(!pass.tls);
    assert!(!pass.dynamic);
    assert_eq!(direct.grpc.headers.len(), 2);
    assert_eq!(direct.grpc.pass_headers, ["X-Accel-Expires"]);
    assert_eq!(
        direct.grpc.ignore_headers,
        ["X-Accel-Redirect", "Cache-Control"]
    );
    assert_eq!(direct.grpc.buffer_size, 8192);
    assert!(!direct.grpc.request_buffering);
    assert!(!direct.grpc.ignore_client_abort);
    assert!(direct.grpc.intercept_errors);
    assert_eq!(direct.grpc.next_upstream.tries, 3);
    assert_eq!(direct.grpc.next_upstream.timeout, Duration::from_secs(2));
    assert_eq!(direct.grpc.bind.as_deref(), Some("127.0.0.2"));
    assert!(direct.grpc.socket_keepalive);
    assert_eq!(direct.grpc.socket_rcvbuf, 32 * 1024);
    assert_eq!(direct.grpc.socket_sndbuf, 64 * 1024);

    let named = server.locations[1].context.grpc.pass.as_ref().unwrap();
    assert_eq!(named.upstream.as_deref(), Some("grpc_backend"));
    assert_eq!(named.authority, "grpc_backend");
    assert!(server.locations[1].context.grpc.request_buffering);

    let tls = server.locations[2].context.grpc.clone();
    assert!(tls.pass.as_ref().unwrap().tls);
    assert_eq!(tls.pass.as_ref().unwrap().port, 443);
    assert_eq!(tls.pass.as_ref().unwrap().authority, "backend.example.test");
    assert!(tls.ssl_server_name);
    assert_eq!(tls.ssl_name.as_deref(), Some("grpc.example.test"));
    assert!(!tls.ssl_session_reuse);
    assert!(tls.ssl_verify);
    assert_eq!(tls.ssl_verify_depth, 4);
    assert!(tls.ssl_trusted_certificate.is_some());
    assert!(tls.ssl_crl.is_some());
    assert!(tls.ssl_certificate.is_some());
    assert!(tls.ssl_certificate_key.is_some());
    assert_eq!(tls.ssl_passwords.len(), 1);

    let dynamic = server.locations[3].context.grpc.pass.as_ref().unwrap();
    assert!(dynamic.dynamic);
    assert_eq!(dynamic.raw, "$grpc_backend");
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn grpc_rejects_invalid_scope_duplicates_and_unsupported_tls_commands() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (config_text, message) in [
        (
            "events {} http { server { grpc_pass 127.0.0.1:9000; } }",
            "not allowed here",
        ),
        (
            "events {} http { server { location / { grpc_pass 127.0.0.1:9000; grpc_pass 127.0.0.1:9001; } } }",
            "duplicate",
        ),
        (
            "events {} http { grpc_buffer_size 0; }",
            "greater than zero",
        ),
        (
            "events {} http { grpc_request_buffering on; grpc_request_buffering off; }",
            "duplicate",
        ),
        (
            "events {} http { grpc_ignore_client_abort on; grpc_ignore_client_abort off; }",
            "duplicate",
        ),
        (
            "events {} http { grpc_ssl_conf_command Ciphersuites TLS_AES_128_GCM_SHA256; }",
            "not supported by the rustls TLS backend",
        ),
        (
            "events {} http { grpc_ssl_certificate_cache inactive=1h; }",
            "must have the \"max\" parameter",
        ),
    ] {
        fs::write(&config_path, config_text).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(message),
            "expected {message:?}, got {:?}",
            error.message
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
