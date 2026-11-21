use super::*;

#[test]
fn compiles_disable_symlinks_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                disable_symlinks on from=$document_root;
                server {
                    listen 127.0.0.1:18530;
                    location /inherited {}
                    location /off {
                        disable_symlinks off;
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
    assert_eq!(
        server.context.disable_symlinks.mode,
        DisableSymlinksMode::On
    );
    assert_eq!(
        server.context.disable_symlinks.from.as_deref(),
        Some("$document_root")
    );
    assert_eq!(
        server.locations[0].context.disable_symlinks,
        server.context.disable_symlinks
    );
    assert_eq!(
        server.locations[1].context.disable_symlinks.mode,
        DisableSymlinksMode::Off
    );
    assert_eq!(server.locations[1].context.disable_symlinks.from, None);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn disable_symlinks_defaults_and_validation() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18531; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let config = &config.http.as_ref().unwrap().servers[0].context;
    assert_eq!(config.disable_symlinks.mode, DisableSymlinksMode::Off);
    assert_eq!(config.disable_symlinks.from, None);

    for (directive, expected) in [
        ("disable_symlinks maybe;", "invalid parameter \"maybe\""),
        (
            "disable_symlinks from=$document_root;",
            "\"disable_symlinks\" must have \"off\", \"on\" or \"if_not_owner\" parameter",
        ),
        (
            "disable_symlinks off from=$document_root;",
            "\"from=\" cannot be used with \"off\" parameter",
        ),
        ("disable_symlinks on on;", "duplicate parameters \"on on\""),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ server {{ listen 127.0.0.1:18531; {directive} }} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18531;
                    disable_symlinks on;
                    disable_symlinks off;
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
            .contains("\"disable_symlinks\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
