use super::*;

#[test]
fn compiles_dav_directives_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                dav_methods put delete;
                dav_access user:rw group:r all:r;
                create_full_put_path on;
                min_delete_depth 2;
                server {
                    listen 127.0.0.1:18525;
                    location /inherited {}
                    location /overridden {
                        dav_methods mkcol move;
                        dav_access group:r;
                        create_full_put_path off;
                        min_delete_depth 0;
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
        server.context.dav.methods,
        vec![DavMethod::Put, DavMethod::Delete]
    );
    assert_eq!(server.context.dav.access, 0o644);
    assert!(server.context.dav.create_full_put_path);
    assert_eq!(server.context.dav.min_delete_depth, 2);

    let inherited = &server.locations[0].context.dav;
    assert_eq!(inherited.methods, vec![DavMethod::Put, DavMethod::Delete]);
    assert_eq!(inherited.access, 0o644);

    let overridden = &server.locations[1].context.dav;
    assert_eq!(overridden.methods, vec![DavMethod::MkCol, DavMethod::Move]);
    assert_eq!(overridden.access, 0o640);
    assert!(!overridden.create_full_put_path);
    assert_eq!(overridden.min_delete_depth, 0);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn defaults_and_rejects_invalid_dav_directives() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18526; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let dav = &config.http.as_ref().unwrap().servers[0].context.dav;
    assert!(dav.methods.is_empty());
    assert_eq!(dav.access, 0o600);
    assert!(!dav.create_full_put_path);
    assert_eq!(dav.min_delete_depth, 0);

    for (directive, expected) in [
        ("dav_methods trace;", "invalid value \"trace\""),
        ("dav_access user:rx;", "invalid value \"user:rx\""),
        (
            "create_full_put_path maybe;",
            "invalid value \"maybe\" in \"create_full_put_path\" directive",
        ),
        (
            "min_delete_depth nope;",
            "invalid value \"nope\" in \"min_delete_depth\" directive",
        ),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ server {{ listen 127.0.0.1:18526; {directive} }} }}\n"),
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
                    listen 127.0.0.1:18526;
                    dav_access user:rw;
                    dav_access group:r;
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
            .contains("\"dav_access\" directive is duplicate"),
        "{error}"
    );
    let _ = fs::remove_dir_all(prefix);
}
