use super::*;

#[test]
fn compiles_server_tokens_build_with_scope_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server_tokens build;
                server {
                    listen 127.0.0.1:18565;
                    location /inherited {}
                    location /off { server_tokens off; }
                    location /on { server_tokens on; }
                }
                server {
                    listen 127.0.0.1:18566;
                    server_tokens on;
                    location /build { server_tokens build; }
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
    let server = &http.servers[0];
    assert_eq!(server.context.server_tokens, ServerTokens::Build);
    assert_eq!(
        server.locations[0].context.server_tokens,
        ServerTokens::Build
    );
    assert_eq!(server.locations[1].context.server_tokens, ServerTokens::Off);
    assert_eq!(server.locations[2].context.server_tokens, ServerTokens::On);
    assert_eq!(http.servers[1].context.server_tokens, ServerTokens::On);
    assert_eq!(
        http.servers[1].locations[0].context.server_tokens,
        ServerTokens::Build
    );

    for (source, expected) in [
        (
            "events {} http { server_tokens sometimes; }",
            "invalid value \"sometimes\"",
        ),
        (
            "events {} http { server_tokens on; server_tokens off; }",
            "\"server_tokens\" directive is duplicate",
        ),
        (
            "events {} http { server_tokens; }",
            "invalid number of arguments in \"server_tokens\" directive",
        ),
    ] {
        fs::write(&config_path, source).unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}
