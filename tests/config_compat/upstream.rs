use super::*;

#[test]
fn compiles_connection_timeouts_and_keepalive_request_limit() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                keepalive_requests 10;
                client_header_timeout 5s;
                client_body_timeout 7s;
                send_timeout 9s;
                server {
                    listen 127.0.0.1:18130;
                    keepalive_requests 3;
                    keepalive_timeout 20s 10s;
                    keepalive_time 30m;
                    location / {
                        client_body_timeout 11s;
                        send_timeout 12s;
                        return 204;
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
    assert_eq!(server.context.keepalive_requests, 3);
    assert_eq!(server.context.client_header_timeout, Duration::from_secs(5));
    assert_eq!(server.context.client_body_timeout, Duration::from_secs(7));
    assert_eq!(server.context.send_timeout, Duration::from_secs(9));
    assert_eq!(server.context.keepalive_timeout, Duration::from_secs(20));
    assert_eq!(server.context.keepalive_time, Duration::from_secs(30 * 60));
    let location = &server.locations[0].context;
    assert_eq!(location.keepalive_requests, 3);
    assert_eq!(location.client_body_timeout, Duration::from_secs(11));
    assert_eq!(location.send_timeout, Duration::from_secs(12));

    fs::write(
        &config_path,
        "events {} http { keepalive_requests nope; }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("keepalive_requests"));

    fs::write(
        &config_path,
        "events {} http { server { location / { client_header_timeout 1s; } } }\n",
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("client_header_timeout"));
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn compiles_upstream_keepalive_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream backend {
                    server 127.0.0.1:8080;
                    keepalive 16 local;
                    keepalive_time 30m;
                    keepalive_timeout 15s;
                    keepalive_requests 250;
                }
                server {
                    listen 127.0.0.1:18131;
                    location / { proxy_pass http://backend; }
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let keepalive = config
        .http
        .as_ref()
        .unwrap()
        .upstreams
        .get("backend")
        .unwrap()
        .keepalive
        .as_ref()
        .unwrap();
    assert_eq!(keepalive.max_cached, 16);
    assert_eq!(keepalive.requests, 250);
    assert_eq!(keepalive.time, Duration::from_secs(30 * 60));
    assert_eq!(keepalive.timeout, Duration::from_secs(15));
    assert!(keepalive.local);

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream backend {
                    server 127.0.0.1:8080;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path.clone()).unwrap();
    let keepalive = config
        .http
        .as_ref()
        .unwrap()
        .upstreams
        .get("backend")
        .unwrap()
        .keepalive
        .as_ref()
        .unwrap();
    assert_eq!(keepalive.max_cached, 32);
    assert_eq!(keepalive.requests, 1000);
    assert_eq!(keepalive.time, Duration::from_secs(3600));
    assert_eq!(keepalive.timeout, Duration::from_secs(60));
    assert!(keepalive.local);

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream backend {
                    server 127.0.0.1:8080;
                    keepalive 16 invalid;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("invalid parameter \"invalid\""));

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream backend {
                    server 127.0.0.1:8080;
                    keepalive 1;
                    keepalive 2;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
    assert!(error.message.contains("duplicate \"keepalive\" directive"));

    fs::write(
        &config_path,
        r#"
            events {}
            http {
                upstream backend {
                    server 127.0.0.1:8080;
                    keepalive 1;
                    keepalive_timeout nope;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let error = compile(document, prefix.clone(), config_path).unwrap_err();
    assert!(error.message.contains("invalid time value \"nope\""));
    let _ = fs::remove_dir_all(prefix);
}
