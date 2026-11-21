use super::*;

#[test]
fn compiles_proxy_pass_trailers_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_pass_trailers on;
                server {
                    listen 127.0.0.1:18496;
                    proxy_pass_trailers off;
                    location /inherited {
                        proxy_pass http://127.0.0.1:8080;
                    }
                    location /on {
                        proxy_pass http://127.0.0.1:8080;
                        proxy_pass_trailers on;
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

    assert!(!server.context.proxy_pass_trailers);
    assert!(!server.locations[0].context.proxy_pass_trailers);
    assert!(server.locations[1].context.proxy_pass_trailers);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_proxy_pass_trailers_values() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { proxy_pass_trailers sometimes; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error.message.contains("invalid value \"sometimes\""),
        "unexpected error: {}",
        error.message
    );
    let _ = fs::remove_dir_all(prefix);
}
