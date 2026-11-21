use super::*;

#[test]
fn compiles_max_ranges_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                max_ranges 3;
                server {
                    listen 127.0.0.1:18560;
                    location /inherit {}
                    location /disabled {
                        max_ranges 0;
                    }
                    location /short {
                        max_ranges 1;
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
    assert_eq!(server.context.max_ranges, 3);
    assert_eq!(server.locations[0].context.max_ranges, 3);
    assert_eq!(server.locations[1].context.max_ranges, 0);
    assert_eq!(server.locations[2].context.max_ranges, 1);

    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18560; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].context.max_ranges,
        i32::MAX as u64
    );

    for (directive, expected) in [
        (
            "max_ranges nope;",
            "invalid number \"nope\" in \"max_ranges\" directive",
        ),
        (
            "max_ranges -1;",
            "invalid number \"-1\" in \"max_ranges\" directive",
        ),
        (
            "max_ranges;",
            "invalid number of arguments in \"max_ranges\" directive",
        ),
        (
            "max_ranges 1; max_ranges 2;",
            "\"max_ranges\" directive is duplicate",
        ),
    ] {
        fs::write(
            &config_path,
            format!(
                "events {{}} http {{ server {{ listen 127.0.0.1:18560; location / {{ {directive} }} }} }}"
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
