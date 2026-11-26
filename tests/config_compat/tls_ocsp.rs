use super::*;

#[test]
fn compiles_http_and_stream_client_ocsp_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                ssl_ocsp leaf;
                ssl_ocsp_responder https://ocsp.example.test/check;
                ssl_ocsp_cache shared:OCSP:1m;
                server {
                    listen 127.0.0.1:18475 ssl;
                    ssl_verify_client on;
                    ssl_client_certificate client-ca.pem;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                }
            }
            stream {
                ssl_ocsp on;
                ssl_ocsp_responder http://127.0.0.1:8080/ocsp;
                ssl_ocsp_cache off;
                server {
                    listen 127.0.0.1:18476 ssl;
                    ssl_verify_client optional;
                    ssl_client_certificate client-ca.pem;
                    ssl_certificate cert.pem;
                    ssl_certificate_key key.pem;
                    return ok;
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
    assert_eq!(http.ssl_ocsp, OcspMode::Leaf);
    assert_eq!(
        http.ssl_ocsp_responder.as_deref(),
        Some("https://ocsp.example.test/check")
    );
    assert_eq!(http.ssl_ocsp_cache.as_deref(), Some("shared:OCSP:1m"));
    let stream = &config.stream.as_ref().unwrap().servers[0];
    assert_eq!(stream.ssl_ocsp, OcspMode::On);
    assert_eq!(
        stream.ssl_ocsp_responder.as_deref(),
        Some("http://127.0.0.1:8080/ocsp")
    );
    assert_eq!(stream.ssl_ocsp_cache, None);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_client_ocsp_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { ssl_ocsp maybe; }",
            "invalid value \"maybe\" in \"ssl_ocsp\" directive",
        ),
        (
            "events {} http { ssl_ocsp_responder ldap://ocsp.example.test; }",
            "invalid OCSP responder URL",
        ),
        (
            "events {} http { ssl_ocsp_cache shared:OCSP; }",
            "expected \"off\" or \"shared:name:size\"",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18477 ssl; \
             ssl_verify_client optional_no_ca; ssl_ocsp on; } }",
            "\"ssl_ocsp\" is incompatible with \"ssl_verify_client optional_no_ca\"",
        ),
        (
            "events {} http { ssl_ocsp on; ssl_ocsp leaf; }",
            "\"ssl_ocsp\" directive is duplicate",
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
