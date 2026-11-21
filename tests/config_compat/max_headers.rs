use super::*;

#[test]
fn compiles_max_headers_with_http_and_server_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                max_headers 7;
                server {
                    listen 127.0.0.1:18562;
                    max_headers 3;
                    location /inherit {}
                }
                server {
                    listen 127.0.0.1:18563;
                    location /main {}
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let http = config.http.as_ref().unwrap();
    assert_eq!(http.servers[0].context.max_headers, 3);
    assert_eq!(http.servers[0].locations[0].context.max_headers, 3);
    assert_eq!(http.servers[1].context.max_headers, 7);

    fs::write(
        &config_path,
        "events {} http { server { listen 127.0.0.1:18562; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].context.max_headers,
        1000
    );

    for (directive, expected) in [
        (
            "http { max_headers nope; }",
            "invalid number \"nope\" in \"max_headers\" directive",
        ),
        (
            "http { max_headers -1; }",
            "invalid number \"-1\" in \"max_headers\" directive",
        ),
        (
            "http { max_headers; }",
            "invalid number of arguments in \"max_headers\" directive",
        ),
        (
            "http { max_headers 1; max_headers 2; }",
            "\"max_headers\" directive is duplicate",
        ),
        (
            "http { server { listen 127.0.0.1:18562; location / { max_headers 1; } } }",
            "\"max_headers\" directive is not allowed here",
        ),
    ] {
        fs::write(&config_path, format!("events {{}} {directive}\n")).unwrap();
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
