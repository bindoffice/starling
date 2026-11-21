use super::*;

#[test]
fn compiles_charset_configuration_and_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                charset_map koi8-r utf-8 {
                    C1 D0B0;
                }
                source_charset koi8-r;
                override_charset on;
                charset_types text/plain application/json;
                server {
                    listen 127.0.0.1:18540;
                    charset utf-8;
                    location /inherited {}
                    location /off {
                        charset off;
                    }
                    location /same {
                        source_charset utf-8;
                        charset utf-8;
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
    assert_eq!(server.context.charset.charset.as_deref(), Some("utf-8"));
    assert_eq!(
        server.context.charset.source_charset.as_deref(),
        Some("koi8-r")
    );
    assert!(server.context.charset.override_charset);
    assert_eq!(
        server.context.charset.types,
        ["text/plain", "application/json"]
    );
    assert_eq!(server.context.charset_maps.len(), 1);
    assert_eq!(server.context.charset_maps[0].source, "koi8-r");
    assert_eq!(server.context.charset_maps[0].destination, "utf-8");

    assert_eq!(server.locations[0].context.charset, server.context.charset);
    assert_eq!(server.locations[1].context.charset.charset, None);
    assert_eq!(
        server.locations[2]
            .context
            .charset
            .source_charset
            .as_deref(),
        Some("utf-8")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_charset_directives_and_maps() {
    for (directive, expected) in [
        (
            "charset_map utf-8 koi8-r {}",
            "\"charset_map\" with \"utf-8\" charset should be given in the second column",
        ),
        (
            "charset_map koi8-r koi8-r {}",
            "\"charset_map\" between the same charsets",
        ),
        (
            "charset_map koi8-r utf-8 { ZZ D0B0; }",
            "invalid value \"ZZ\"",
        ),
        (
            "charset_map koi8-r utf-8 { C1 D0B; }",
            "invalid value \"D0B\"",
        ),
        (
            "charset_map koi8-r utf-8 { C1 F09F9880; }",
            "invalid value \"F09F9880\"",
        ),
    ] {
        let error = compile_error(&format!(
            "events {{}} http {{ {directive} server {{ listen 127.0.0.1:18541; }} }}"
        ));
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn rejects_duplicate_charset_maps_and_missing_conversions() {
    let duplicate = compile_error(
        r#"
            events {}
            http {
                charset_map koi8-r windows-1251 {}
                charset_map windows-1251 koi8-r {}
                server { listen 127.0.0.1:18542; }
            }
        "#,
    );
    assert!(
        duplicate.contains("duplicate \"charset_map\" between"),
        "{duplicate}"
    );

    let missing = compile_error(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18543;
                    source_charset koi8-r;
                    charset utf-8;
                }
            }
        "#,
    );
    assert!(
        missing.contains("no \"charset_map\" between the charsets \"koi8-r\" and \"utf-8\""),
        "{missing}"
    );
}

fn compile_error(source: &str) -> String {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path)
        .unwrap_err()
        .to_string();
    let _ = fs::remove_dir_all(prefix);
    error
}
