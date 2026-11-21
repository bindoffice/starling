use super::*;

#[test]
fn compiles_client_certificate_verification_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_verify_client on;
                ssl_verify_depth 2;
                ssl_trusted_certificate client-ca.pem;
                ssl_crl client.crl;
                server {
                    listen 127.0.0.1:18467 ssl;
                    location / { return 204; }
                }
                server {
                    listen 127.0.0.1:18468 ssl;
                    ssl_verify_client optional_no_ca;
                    ssl_verify_depth 0;
                    ssl_crl server.crl;
                    location / { return 204; }
                }
            }
            stream {
                ssl_verify_client optional;
                ssl_client_certificate stream-ca.pem;
                ssl_crl stream.crl;
                server {
                    listen 127.0.0.1:18469 ssl;
                    ssl_crl stream-server.crl;
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
    let http = config.http.as_ref().unwrap();
    assert_eq!(
        http.servers[0].context.ssl_verify_client,
        ClientCertificateVerify::On
    );
    assert_eq!(http.servers[0].context.ssl_verify_depth, 2);
    assert_eq!(
        http.servers[0].context.ssl_trusted_certificate,
        Some(prefix.join("client-ca.pem"))
    );
    assert_eq!(
        http.servers[0].context.ssl_crl,
        Some(prefix.join("client.crl"))
    );
    assert_eq!(
        http.servers[1].context.ssl_verify_client,
        ClientCertificateVerify::OptionalNoCa
    );
    assert_eq!(http.servers[1].context.ssl_verify_depth, 0);
    assert_eq!(
        http.servers[1].context.ssl_trusted_certificate,
        Some(prefix.join("client-ca.pem"))
    );
    assert_eq!(
        http.servers[1].context.ssl_crl,
        Some(prefix.join("server.crl"))
    );

    let stream = config.stream.as_ref().unwrap();
    assert_eq!(
        stream.servers[0].ssl_verify_client,
        ClientCertificateVerify::Optional
    );
    assert_eq!(stream.servers[0].ssl_verify_depth, 1);
    assert_eq!(
        stream.servers[0].ssl_client_certificate,
        Some(prefix.join("stream-ca.pem"))
    );
    assert_eq!(
        stream.servers[0].ssl_crl,
        Some(prefix.join("stream-server.crl"))
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_client_certificate_verification_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18470 ssl; ssl_verify_client on; } }",
            "no ssl_client_certificate or ssl_trusted_certificate",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18470 ssl; ssl_verify_client invalid; } }",
            "invalid value",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18470 ssl; ssl_verify_depth nope; } }",
            "invalid value",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18470 ssl; location / { ssl_verify_client off; } } }",
            "not allowed here",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18470 ssl; location / { ssl_crl client.crl; } } }",
            "not allowed here",
        ),
        (
            "events {} stream { server { listen 127.0.0.1:18470 ssl; ssl_verify_client optional; proxy_pass 127.0.0.1:19443; } }",
            "no ssl_client_certificate or ssl_trusted_certificate",
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
