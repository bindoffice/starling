use super::*;

#[test]
fn compiles_chunked_transfer_encoding_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                chunked_transfer_encoding off;
                server {
                    listen 127.0.0.1:18513;
                    location /inherit {}
                    location /enabled {
                        chunked_transfer_encoding on;
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
    let server = &config.http.as_ref().unwrap().servers[0];
    assert!(!server.context.chunked_transfer_encoding);
    assert!(!server.locations[0].context.chunked_transfer_encoding);
    assert!(server.locations[1].context.chunked_transfer_encoding);

    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18513; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert!(
        config.http.as_ref().unwrap().servers[0]
            .context
            .chunked_transfer_encoding
    );

    fs::write(
        &config_path,
        "events {} http { chunked_transfer_encoding sometimes; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(
        error.message.contains("must be \"on\" or \"off\""),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
