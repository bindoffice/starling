use super::*;

#[test]
fn compiles_and_inherits_uwsgi_string() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                uwsgi_string "http=$host";
                server {
                    listen 127.0.0.1:18520;
                    uwsgi_string "server=$host";
                    location /inherited {
                        uwsgi_pass 127.0.0.1:9001;
                    }
                    location /override {
                        uwsgi_string "location=$host";
                        uwsgi_pass 127.0.0.1:9001;
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
    assert_eq!(server.context.uwsgi_string, "server=$host");
    assert_eq!(server.locations[0].context.uwsgi_string, "server=$host");
    assert_eq!(server.locations[1].context.uwsgi_string, "location=$host");

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_uwsgi_string_usage() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (source, expected) in [
        (
            "events {} http { uwsgi_string one; uwsgi_string two; }",
            "is duplicate",
        ),
        (
            "events {} http { uwsgi_string; }",
            "invalid number of arguments",
        ),
        (
            "events {} http { uwsgi_string one two; }",
            "invalid number of arguments",
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
