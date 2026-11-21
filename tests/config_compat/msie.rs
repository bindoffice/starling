use super::*;

#[test]
fn compiles_msie_directives_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                msie_padding off;
                msie_refresh on;
                server {
                    listen 127.0.0.1:18523;
                    location /inherited {}
                    location /overridden {
                        msie_padding on;
                        msie_refresh off;
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
    assert!(!server.context.msie_padding);
    assert!(server.context.msie_refresh);
    assert!(!server.locations[0].context.msie_padding);
    assert!(server.locations[0].context.msie_refresh);
    assert!(server.locations[1].context.msie_padding);
    assert!(!server.locations[1].context.msie_refresh);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_msie_directives_and_rejects_duplicates() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18524; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let context = &config.http.as_ref().unwrap().servers[0].context;
    assert!(context.msie_padding);
    assert!(!context.msie_refresh);

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18524;
                    msie_padding on;
                    msie_padding off;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error
            .message
            .contains("\"msie_padding\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
