use super::*;

#[test]
fn compiles_upstream_force_ranges_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_force_ranges on;
                server {
                    listen 127.0.0.1:18146;
                    fastcgi_force_ranges on;
                    uwsgi_force_ranges on;
                    scgi_force_ranges on;
                    location /inherited {
                        proxy_pass http://127.0.0.1:9000;
                    }
                    location /overridden {
                        proxy_pass http://127.0.0.1:9000;
                        proxy_force_ranges off;
                        fastcgi_force_ranges off;
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
    assert!(server.context.proxy_force_ranges);
    assert!(server.context.fastcgi_force_ranges);
    assert!(server.context.uwsgi_force_ranges);
    assert!(server.context.scgi_force_ranges);
    assert!(server.locations[0].context.proxy_force_ranges);
    let overridden = &server.locations[1].context;
    assert!(!overridden.proxy_force_ranges);
    assert!(!overridden.fastcgi_force_ranges);
    assert!(overridden.uwsgi_force_ranges);
    assert!(overridden.scgi_force_ranges);

    for source in [
        "events {} http { proxy_force_ranges; }",
        "events {} http { proxy_force_ranges maybe; }",
        "events {} http { fastcgi_force_ranges maybe; }",
        "events {} http { uwsgi_force_ranges maybe; }",
        "events {} http { scgi_force_ranges maybe; }",
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains("invalid value")
                || error.message.contains("invalid number of arguments"),
            "{error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
