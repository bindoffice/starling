use super::*;

#[test]
fn compiles_lingering_close_and_timeout_reset_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                lingering_close always;
                lingering_time 12s;
                lingering_timeout 750ms;
                reset_timedout_connection on;
                server {
                    listen 127.0.0.1:18568;
                    location /inherited {}
                    location /off {
                        lingering_close off;
                        lingering_time 3s;
                        lingering_timeout 0;
                        reset_timedout_connection off;
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
    assert_eq!(server.context.lingering_close, LingeringClose::Always);
    assert_eq!(server.context.lingering_time, Duration::from_secs(12));
    assert_eq!(server.context.lingering_timeout, Duration::from_millis(750));
    assert!(server.context.reset_timedout_connection);
    let inherited = &server.locations[0].context;
    assert_eq!(inherited.lingering_close, LingeringClose::Always);
    assert_eq!(inherited.lingering_time, Duration::from_secs(12));
    assert_eq!(inherited.lingering_timeout, Duration::from_millis(750));
    assert!(inherited.reset_timedout_connection);
    let overridden = &server.locations[1].context;
    assert_eq!(overridden.lingering_close, LingeringClose::Off);
    assert_eq!(overridden.lingering_time, Duration::from_secs(3));
    assert_eq!(overridden.lingering_timeout, Duration::ZERO);
    assert!(!overridden.reset_timedout_connection);

    for (source, expected) in [
        (
            "events {} http { lingering_close sometimes; }",
            "invalid value \"sometimes\"",
        ),
        (
            "events {} http { lingering_time nope; }",
            "invalid time value",
        ),
        (
            "events {} http { reset_timedout_connection maybe; }",
            "invalid value \"maybe\"",
        ),
        (
            "events {} http { lingering_close on; lingering_close off; }",
            "\"lingering_close\" directive is duplicate",
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
