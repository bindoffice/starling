use super::*;

#[test]
fn compiles_proxy_send_and_read_timeouts_independently() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                proxy_send_timeout 2s;
                proxy_read_timeout 3s;
                server {
                    listen 127.0.0.1:18560;
                    proxy_send_timeout 4s;
                    location /inherited {}
                    location /override {
                        proxy_send_timeout 5s;
                        proxy_read_timeout 6s;
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
    assert_eq!(server.context.proxy_send_timeout, Duration::from_secs(4));
    assert_eq!(server.context.proxy_read_timeout, Duration::from_secs(3));
    assert_eq!(
        server.locations[0].context.proxy_send_timeout,
        Duration::from_secs(4)
    );
    assert_eq!(
        server.locations[0].context.proxy_read_timeout,
        Duration::from_secs(3)
    );
    assert_eq!(
        server.locations[1].context.proxy_send_timeout,
        Duration::from_secs(5)
    );
    assert_eq!(
        server.locations[1].context.proxy_read_timeout,
        Duration::from_secs(6)
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_proxy_timeout_arguments() {
    for (directive, expected) in [
        (
            "proxy_send_timeout;",
            "invalid number of arguments in \"proxy_send_timeout\" directive",
        ),
        (
            "proxy_send_timeout invalid;",
            "invalid time value \"invalid\"",
        ),
    ] {
        let prefix = temp_prefix();
        fs::create_dir_all(&prefix).unwrap();
        let config_path = prefix.join("nginx.conf");
        fs::write(
            &config_path,
            format!("events {{}} http {{ server {{ listen 127.0.0.1:18561; {directive} }} }}"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
        let _ = fs::remove_dir_all(prefix);
    }
}
