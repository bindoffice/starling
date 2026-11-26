use super::*;

#[test]
fn compiles_http3_directive_and_quic_listener() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                http3 on;
                server {
                    listen 127.0.0.1:18460 quic;
                }
                server {
                    listen 127.0.0.1:18461 quic;
                    http3 off;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let servers = &config.http.as_ref().unwrap().servers;
    assert!(servers[0].http3);
    assert!(servers[0].listens[0].quic);
    assert!(!servers[1].http3);
    assert!(servers[1].listens[0].quic);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_http3_and_quic_listener_combinations() {
    for (source, expected) in [
        (
            "events {} http { server { listen 127.0.0.1:18462 quic; ssl_protocols TLSv1.2; } }",
            "HTTP/3 requires TLSv1.3",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18462 quic proxy_protocol; } }",
            "\"proxy_protocol\" is not supported with \"quic\"",
        ),
        (
            "events {} http { server { listen unix:/tmp/starling-http3.sock quic; } }",
            "\"quic\" is incompatible with UNIX domain listen sockets",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18462 quic; http3 on; http3 off; } }",
            "\"http3\" directive is duplicate",
        ),
        (
            "events {} http { server { listen 127.0.0.1:18462 quic; http3 maybe; } }",
            "it must be \"on\" or \"off\"",
        ),
        (
            "events {} stream { server { listen 127.0.0.1:18462 quic; return ok; } }",
            "invalid parameter \"quic\" in listen directive",
        ),
    ] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {source}: {error}"
        );
        let _ = fs::remove_dir_all(prefix);
    }
}
