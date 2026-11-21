use super::*;

#[test]
fn compiles_proxy_http_version_two() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_http_version 2;
                server {
                    location / {
                        proxy_pass http://127.0.0.1:9000;
                    }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0]
            .context
            .proxy_http_version,
        "2"
    );

    fs::write(&config_path, "events {} http { proxy_http_version 3; }\n").unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("invalid value \"3\""));
    let _ = fs::remove_dir_all(prefix);
}
