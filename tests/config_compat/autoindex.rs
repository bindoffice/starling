use super::*;

#[test]
fn compiles_autoindex_formats_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                autoindex on;
                autoindex_format json;
                autoindex_exact_size off;
                server {
                    listen 127.0.0.1:18460;
                    location /inherited {}
                    location /xml/ {
                        autoindex_format xml;
                        autoindex_localtime on;
                    }
                    location /html/ {
                        autoindex_format html;
                        autoindex_exact_size on;
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

    assert!(server.context.autoindex);
    assert_eq!(server.context.autoindex_format, AutoindexFormat::Json);
    assert!(!server.context.autoindex_exact_size);
    assert_eq!(
        server.locations[0].context.autoindex_format,
        AutoindexFormat::Json
    );
    assert_eq!(
        server.locations[1].context.autoindex_format,
        AutoindexFormat::Xml
    );
    assert!(server.locations[1].context.autoindex_localtime);
    assert_eq!(
        server.locations[2].context.autoindex_format,
        AutoindexFormat::Html
    );
    assert!(server.locations[2].context.autoindex_exact_size);

    for (directive, expected) in [
        (
            "autoindex_format yaml;",
            "invalid value \"yaml\" in \"autoindex_format\" directive",
        ),
        (
            "autoindex_format json; autoindex_format xml;",
            "\"autoindex_format\" directive is duplicate",
        ),
        (
            "autoindex on; autoindex off;",
            "\"autoindex\" directive is duplicate",
        ),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}
