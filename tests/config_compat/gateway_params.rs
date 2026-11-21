use super::*;

#[test]
fn compiles_fastcgi_passes_params_and_includes() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    fs::write(
        prefix.join("fastcgi_params"),
        "fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;\n\
         fastcgi_param QUERY_STRING $query_string;\n",
    )
    .unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                fastcgi_connect_timeout 5s;
                fastcgi_send_timeout 12s;
                fastcgi_read_timeout 30s;
                server {
                    listen 127.0.0.1:18119;
                    location /php/ {
                        include fastcgi_params;
                        fastcgi_pass 127.0.0.1:9000;
                        fastcgi_split_path_info ^(.+?\.php)(/.*)$;
                        fastcgi_pass_request_headers off;
                        fastcgi_param CONTENT_TYPE $content_type if_not_empty;
                        fastcgi_intercept_errors on;
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
    assert_eq!(
        server.context.fastcgi_connect_timeout,
        Duration::from_secs(5)
    );
    assert_eq!(server.context.fastcgi_read_timeout, Duration::from_secs(30));
    assert_eq!(server.context.fastcgi_send_timeout, Duration::from_secs(12));
    let context = &server.locations[0].context;
    assert_eq!(
        context.fastcgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:9000")
    );
    assert_eq!(context.fastcgi_params.len(), 3);
    assert_eq!(context.fastcgi_params[0].name, "SCRIPT_FILENAME");
    assert!(context.fastcgi_params[2].if_not_empty);
    assert!(context.fastcgi_split_path_info.is_some());
    assert!(!context.fastcgi_pass_request_headers);
    assert!(context.fastcgi_intercept_errors);

    fs::write(
        &config_path,
        r#"events {} http { server { fastcgi_split_path_info ^(.+)$; } }"#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("must have 2 captures"));

    fs::write(
        &config_path,
        "events {} http { server { location / { fastcgi_pass unix:/run/php.sock; } } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].locations[0]
            .context
            .fastcgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("unix:/run/php.sock")
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_uwsgi_and_scgi_passes_params_and_includes() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    fs::write(
        prefix.join("uwsgi_params"),
        "uwsgi_param REQUEST_METHOD $request_method;\n\
         uwsgi_param PATH_INFO $document_uri;\n",
    )
    .unwrap();
    fs::write(
        prefix.join("scgi_params"),
        "scgi_param REQUEST_METHOD $request_method;\n\
         scgi_param REQUEST_URI $request_uri;\n",
    )
    .unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                uwsgi_connect_timeout 2s;
                uwsgi_send_timeout 3s;
                scgi_connect_timeout 3s;
                upstream uwsgi_backend {
                    server 127.0.0.1:9001;
                }
                server {
                    listen 127.0.0.1:18122;
                    location /u/ {
                        include uwsgi_params;
                        uwsgi_pass uwsgi_backend;
                        uwsgi_modifier1 9;
                        uwsgi_modifier2 1;
                        uwsgi_pass_request_body off;
                        uwsgi_intercept_errors on;
                    }
                    location /s/ {
                        include scgi_params;
                        scgi_pass 127.0.0.1:9002;
                        scgi_read_timeout 4s;
                        scgi_send_timeout 5s;
                        scgi_pass_request_headers off;
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
    assert_eq!(server.context.uwsgi_connect_timeout, Duration::from_secs(2));
    assert_eq!(server.context.uwsgi_send_timeout, Duration::from_secs(3));
    assert_eq!(server.context.scgi_connect_timeout, Duration::from_secs(3));
    let uwsgi = &server.locations[0].context;
    assert!(matches!(
        uwsgi.uwsgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("uwsgi_backend")
    ));
    assert_eq!(uwsgi.uwsgi_params.len(), 2);
    assert_eq!(uwsgi.uwsgi_modifier1, 9);
    assert_eq!(uwsgi.uwsgi_modifier2, 1);
    assert!(!uwsgi.uwsgi_pass_request_body);
    assert!(uwsgi.uwsgi_intercept_errors);
    let scgi = &server.locations[1].context;
    assert_eq!(
        scgi.scgi_pass.as_ref().map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:9002")
    );
    assert_eq!(scgi.scgi_params.len(), 2);
    assert_eq!(scgi.scgi_read_timeout, Duration::from_secs(4));
    assert_eq!(scgi.scgi_send_timeout, Duration::from_secs(5));
    assert!(!scgi.scgi_pass_request_headers);

    fs::write(
        &config_path,
        "events {} http { server { uwsgi_modifier1 300; } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("uwsgi_modifier1"));

    fs::write(
        &config_path,
        "events {} http { server { location / { scgi_pass unix:/run/scgi.sock; } } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].locations[0]
            .context
            .scgi_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("unix:/run/scgi.sock")
    );
    let _ = fs::remove_dir_all(prefix);
}
