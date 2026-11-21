use super::*;

#[test]
fn compiles_upstream_limit_rates_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_limit_rate 32k;
                server {
                    listen 127.0.0.1:18474;
                    fastcgi_limit_rate 16k;
                    uwsgi_limit_rate $server_port;
                    scgi_limit_rate 4k;
                    location /inherited {
                        proxy_pass http://127.0.0.1:9000;
                    }
                    location /overridden {
                        proxy_pass http://127.0.0.1:9000;
                        proxy_limit_rate 8k;
                        fastcgi_limit_rate 0;
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
    assert_eq!(server.context.proxy_limit_rate.as_deref(), Some("32k"));
    assert_eq!(server.context.fastcgi_limit_rate.as_deref(), Some("16k"));
    assert_eq!(
        server.context.uwsgi_limit_rate.as_deref(),
        Some("$server_port")
    );
    assert_eq!(server.context.scgi_limit_rate.as_deref(), Some("4k"));
    assert_eq!(
        server.locations[0].context.proxy_limit_rate.as_deref(),
        Some("32k")
    );
    assert_eq!(
        server.locations[1].context.proxy_limit_rate.as_deref(),
        Some("8k")
    );
    assert_eq!(
        server.locations[1].context.fastcgi_limit_rate.as_deref(),
        Some("0")
    );
    assert_eq!(
        server.locations[1].context.uwsgi_limit_rate.as_deref(),
        Some("$server_port")
    );

    for source in [
        "events {} http { proxy_limit_rate; }",
        "events {} http { proxy_limit_rate invalid; }",
        "events {} http { fastcgi_limit_rate invalid; }",
        "events {} http { uwsgi_limit_rate invalid; }",
        "events {} http { scgi_limit_rate invalid; }",
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
