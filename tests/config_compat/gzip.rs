use super::*;

#[test]
fn compiles_gzip_http_version_and_disable_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                gzip_http_version 1.0;
                gzip_disable "ParentAgent" msie6;
                server {
                    listen 127.0.0.1:18510;
                    location /inherit {}
                    location /override {
                        gzip_http_version 1.1;
                        gzip_disable "ChildAgent";
                    }
                    location /multiple {
                        gzip_disable "First";
                        gzip_disable "Second";
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

    assert_eq!(server.context.gzip_http_version, GzipHttpVersion::Http10);
    assert!(server.context.gzip_disable.msie6);
    assert_eq!(server.context.gzip_disable.regexes.len(), 1);
    assert!(server.context.gzip_disable.regexes[0].is_match("parentagent"));

    let inherited = &server.locations[0].context;
    assert_eq!(inherited.gzip_http_version, GzipHttpVersion::Http10);
    assert!(inherited.gzip_disable.msie6);
    assert!(inherited.gzip_disable.regexes[0].is_match("PARENTAGENT"));

    let overridden = &server.locations[1].context;
    assert_eq!(overridden.gzip_http_version, GzipHttpVersion::Http11);
    assert!(overridden.gzip_disable.msie6);
    assert_eq!(overridden.gzip_disable.regexes.len(), 1);
    assert!(overridden.gzip_disable.regexes[0].is_match("CHILDAGENT"));
    assert!(!overridden.gzip_disable.regexes[0].is_match("ParentAgent"));

    let multiple = &server.locations[2].context;
    assert_eq!(multiple.gzip_disable.regexes.len(), 2);
    assert!(multiple.gzip_disable.regexes[0].is_match("first"));
    assert!(multiple.gzip_disable.regexes[1].is_match("SECOND"));
    assert!(
        multiple
            .gzip_disable
            .regexes
            .iter()
            .all(|regex| !regex.is_match("ParentAgent"))
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_gzip_version_and_disable_rules() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (directive, expected) in [
        (
            "gzip_http_version 2.0;",
            "invalid value \"2.0\" in \"gzip_http_version\" directive",
        ),
        (
            "gzip_http_version $request_version;",
            "invalid value \"$request_version\"",
        ),
        (
            "gzip_disable \"(\";",
            "invalid gzip_disable regular expression",
        ),
        (
            "gzip_disable degradation;",
            "the \"degradation\" parameter in \"gzip_disable\" is not supported",
        ),
        (
            "gzip_disable;",
            "invalid number of arguments in \"gzip_disable\" directive",
        ),
    ] {
        fs::write(
            &config_path,
            format!(
                "events {{}} http {{ server {{ listen 127.0.0.1:18511; location / {{ {directive} }} }} }}"
            ),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected error for {directive}: {error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}
