use super::*;

#[test]
fn compiles_http_tcp_nodelay_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                tcp_nodelay off;
                server {
                    listen 127.0.0.1:18294;
                    location /inherited {}
                    location /enabled {
                        tcp_nodelay on;
                    }
                }
                server {
                    listen 127.0.0.1:18295;
                    tcp_nodelay on;
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

    assert!(!http.servers[0].context.tcp_nodelay);
    assert!(!http.servers[0].locations[0].context.tcp_nodelay);
    assert!(http.servers[0].locations[1].context.tcp_nodelay);
    assert!(http.servers[1].context.tcp_nodelay);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_http_tcp_nodelay_to_on_and_rejects_invalid_values() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18296; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert!(config.http.as_ref().unwrap().servers[0].context.tcp_nodelay);

    for source in [
        "events {} http { tcp_nodelay maybe; }\n",
        "events {} http { tcp_nodelay on; tcp_nodelay off; }\n",
    ] {
        let error = compile_error(&prefix, source);
        assert!(
            error.contains("tcp_nodelay"),
            "unexpected tcp_nodelay error: {error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

fn compile_error(prefix: &std::path::Path, source: &str) -> String {
    let config_path = prefix.join("invalid.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.to_path_buf())
        .load(&config_path)
        .unwrap();
    compile(document, prefix.to_path_buf(), config_path)
        .expect_err("configuration should fail")
        .to_string()
}
