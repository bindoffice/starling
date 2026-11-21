use super::*;

#[test]
fn compiles_memcached_directives_with_inheritance() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                memcached_bind 127.0.0.1;
                memcached_socket_keepalive on;
                memcached_connect_timeout 5s;
                memcached_send_timeout 6s;
                memcached_read_timeout 7s;
                memcached_buffer_size 8k;
                memcached_next_upstream error timeout invalid_response not_found;
                memcached_next_upstream_tries 3;
                memcached_next_upstream_timeout 9s;
                memcached_gzip_flag 16;
                server {
                    listen 127.0.0.1:18144;
                    location /cache/ {
                        set $memcached_key $uri;
                        memcached_pass 127.0.0.1:11211;
                    }
                    location /override/ {
                        memcached_bind $remote_addr;
                        memcached_socket_keepalive off;
                        memcached_connect_timeout 1s;
                        memcached_send_timeout 2s;
                        memcached_read_timeout 3s;
                        memcached_buffer_size 16k;
                        memcached_next_upstream off;
                        memcached_gzip_flag 32;
                        memcached_pass unix:/run/memcached.sock;
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
    let location = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    assert_eq!(
        location
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("127.0.0.1:11211")
    );
    assert_eq!(location.memcached_bind.as_deref(), Some("127.0.0.1"));
    assert!(location.memcached_socket_keepalive);
    assert_eq!(location.memcached_connect_timeout, Duration::from_secs(5));
    assert_eq!(location.memcached_send_timeout, Duration::from_secs(6));
    assert_eq!(location.memcached_read_timeout, Duration::from_secs(7));
    assert_eq!(location.memcached_buffer_size, 8 * 1024);
    assert!(location.memcached_next_upstream.retry_error());
    assert!(location.memcached_next_upstream.retry_timeout());
    assert!(location.memcached_next_upstream.retry_invalid_header());
    assert!(location.memcached_next_upstream.retry_status(404));
    assert_eq!(location.memcached_next_upstream.tries, 3);
    assert_eq!(
        location.memcached_next_upstream.timeout,
        Duration::from_secs(9)
    );
    assert_eq!(location.memcached_gzip_flag, 16);

    let overridden = &config.http.as_ref().unwrap().servers[0].locations[1].context;
    assert_eq!(
        overridden
            .memcached_pass
            .as_ref()
            .map(|pass| pass.raw.as_str()),
        Some("unix:/run/memcached.sock")
    );
    assert_eq!(overridden.memcached_bind.as_deref(), Some("$remote_addr"));
    assert!(!overridden.memcached_socket_keepalive);
    assert_eq!(overridden.memcached_connect_timeout, Duration::from_secs(1));
    assert_eq!(overridden.memcached_send_timeout, Duration::from_secs(2));
    assert_eq!(overridden.memcached_read_timeout, Duration::from_secs(3));
    assert_eq!(overridden.memcached_buffer_size, 16 * 1024);
    assert!(!overridden.memcached_next_upstream.retry_error());
    assert!(!overridden.memcached_next_upstream.retry_timeout());
    assert_eq!(overridden.memcached_gzip_flag, 32);

    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_memcached_configuration() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    for (directive, expected) in [
        (
            "server { memcached_pass 127.0.0.1:11211; }",
            "directive is not allowed here",
        ),
        (
            "server { location / { memcached_pass 127.0.0.1:11211; memcached_pass 127.0.0.1:11212; } }",
            "directive is duplicate",
        ),
        (
            "server { location / { memcached_pass http://127.0.0.1:11211; } }",
            "invalid URL prefix",
        ),
        (
            "server { location / { memcached_pass unix:; } }",
            "invalid UNIX domain socket path",
        ),
        ("memcached_buffer_size 0;", "invalid value \"0\""),
        ("memcached_gzip_flag invalid;", "invalid value \"invalid\""),
        (
            "memcached_next_upstream http_500;",
            "invalid parameter \"http_500\"",
        ),
        (
            "memcached_bind 127.0.0.1 transparent;",
            "transparent proxying is not supported",
        ),
    ] {
        fs::write(
            &config_path,
            format!("events {{}} http {{ {directive} }}\n"),
        )
        .unwrap();
        let document = ConfigLoader::new(prefix.clone())
            .load(&config_path)
            .unwrap();
        let error = compile(document, prefix.clone(), config_path.clone()).unwrap_err();
        assert!(error.message.contains(expected), "{error}");
    }
    let _ = fs::remove_dir_all(prefix);
}
