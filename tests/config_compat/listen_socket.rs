use super::*;

#[test]
fn compiles_http_listen_socket_options() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18300
                        backlog=1024
                        rcvbuf=32k
                        sndbuf=64k
                        reuseport
                        bind
                        so_keepalive=30s:5s:3
                        fastopen=16
                        deferred;
                }
                server {
                    listen 127.0.0.1:18301 so_keepalive=on;
                }
                server {
                    listen [::1]:18302 ipv6only=off so_keepalive=off;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let servers = &config.http.as_ref().unwrap().servers;

    let options = &servers[0].listens[0].socket_options;
    assert!(servers[0].listens[0].socket_options_set);
    assert_eq!(options.backlog, 1024);
    assert_eq!(options.receive_buffer_size, Some(32 * 1024));
    assert_eq!(options.send_buffer_size, Some(64 * 1024));
    assert!(options.reuseport);
    assert!(options.bind);
    assert_eq!(
        options.keepalive,
        ListenKeepalive::Tuned {
            idle: Some(Duration::from_secs(30)),
            interval: Some(Duration::from_secs(5)),
            count: Some(3),
        }
    );
    assert_eq!(options.fastopen, Some(16));
    assert!(options.deferred);
    assert_eq!(
        servers[1].listens[0].socket_options.keepalive,
        ListenKeepalive::On
    );
    assert!(!servers[2].listens[0].socket_options.ipv6only);
    assert_eq!(
        servers[2].listens[0].socket_options.keepalive,
        ListenKeepalive::Off
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn applies_listen_socket_options_to_stream_and_defaults() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            stream {
                server {
                    listen 127.0.0.1:18303 rcvbuf=16k reuseport;
                    return ok;
                }
            }
            http {
                server {}
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let stream_options = &config.stream.as_ref().unwrap().servers[0].listens[0].socket_options;
    assert_eq!(stream_options.receive_buffer_size, Some(16 * 1024));
    assert!(stream_options.reuseport);

    let default = &config.http.as_ref().unwrap().servers[0].listens[0];
    assert_eq!(default.socket_options, ListenSocketOptions::default());
    assert!(!default.socket_options_set);
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn expands_ipv4_and_ipv6_listen_port_ranges() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18310-18312 reuseport;
                    listen [::1]:18310-18312;
                }
            }
            stream {
                server {
                    listen 127.0.0.1:18313-18314;
                    return ok;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    let listens = &config.http.as_ref().unwrap().servers[0].listens;
    assert_eq!(listens.len(), 6);
    assert_eq!(
        listens
            .iter()
            .take(3)
            .map(|listen| listen.address.to_string())
            .collect::<Vec<_>>(),
        ["127.0.0.1:18310", "127.0.0.1:18311", "127.0.0.1:18312"]
    );
    assert!(
        listens
            .iter()
            .take(3)
            .all(|listen| { listen.socket_options.reuseport && listen.socket_options_set })
    );
    assert_eq!(
        listens
            .iter()
            .skip(3)
            .map(|listen| listen.address.to_string())
            .collect::<Vec<_>>(),
        ["[::1]:18310", "[::1]:18311", "[::1]:18312"]
    );
    assert_eq!(config.stream.as_ref().unwrap().servers[0].listens.len(), 2);

    for range in ["18312-18310", "0-1", "65535-65536"] {
        let error = compile_error(
            &format!("events {{}} http {{ server {{ listen 127.0.0.1:{range}; }} }}"),
            &compile_prefix(),
        );
        assert!(
            error.contains("invalid port") || error.contains("invalid port range"),
            "unexpected error for {range}: {error}"
        );
    }
    let _ = fs::remove_dir_all(prefix);
}

#[cfg(target_os = "linux")]
#[test]
fn compiles_multipath_listener_on_linux() {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    let config_path = prefix.join("nginx.conf");
    fs::write(
        &config_path,
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18307 multipath;
                }
            }
            stream {
                server {
                    listen 127.0.0.1:18308 multipath;
                    return ok;
                }
            }
        "#,
    )
    .unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .unwrap();
    let config = compile(document, prefix.clone(), config_path).unwrap();
    assert!(
        config.http.as_ref().unwrap().servers[0].listens[0]
            .socket_options
            .multipath
    );
    assert!(
        config.stream.as_ref().unwrap().servers[0].listens[0]
            .socket_options
            .multipath
    );
    let _ = fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_and_duplicate_listen_socket_options() {
    for (listen, expected) in [
        ("listen 127.0.0.1:18304 backlog=0;", "invalid backlog"),
        ("listen 127.0.0.1:18304 backlog=bad;", "invalid backlog"),
        ("listen 127.0.0.1:18304 rcvbuf=bad;", "invalid rcvbuf"),
        ("listen 127.0.0.1:18304 ipv6only=maybe;", "invalid ipv6only"),
        (
            "listen 127.0.0.1:18304 so_keepalive=0:0:0;",
            "invalid so_keepalive",
        ),
        (
            "listen 127.0.0.1:18304 so_keepalive=1s:2s:3:4;",
            "invalid so_keepalive",
        ),
        ("listen 127.0.0.1:18304 fastopen=bad;", "invalid fastopen"),
        ("listen 127.0.0.1:18304 setfib=1;", "invalid parameter"),
    ] {
        let error = compile_error(
            &format!("events {{}} http {{ server {{ {listen} }} }}"),
            &compile_prefix(),
        );
        assert!(
            error.contains(expected),
            "unexpected error for {listen}: {error}"
        );
    }

    let duplicate_options = compile_error(
        r#"
            events {}
            http {
                server { listen 127.0.0.1:18305 backlog=10; }
                server { listen 127.0.0.1:18305 backlog=20; }
            }
        "#,
        &compile_prefix(),
    );
    assert!(duplicate_options.contains("duplicate listen options"));

    let duplicate_default = compile_error(
        r#"
            events {}
            http {
                server { listen 127.0.0.1:18306 default_server; }
                server { listen 127.0.0.1:18306 default_server; }
            }
        "#,
        &compile_prefix(),
    );
    assert!(duplicate_default.contains("duplicate default server"));
}

#[test]
fn rejects_tcp_only_options_on_udp_stream_listeners() {
    for (option, expected) in [
        ("fastopen=1", "fastopen"),
        ("backlog=10", "backlog"),
        ("deferred", "deferred"),
        ("so_keepalive=on", "so_keepalive"),
    ] {
        let error = compile_error(
            &format!("events {{}} stream {{ server {{ listen 127.0.0.1:18309 udp {option}; }} }}"),
            &compile_prefix(),
        );
        assert!(
            error.contains(expected) && error.contains("incompatible with \"udp\""),
            "unexpected error for {option}: {error}"
        );
    }

    #[cfg(target_os = "linux")]
    {
        let error = compile_error(
            "events {} stream { server { listen 127.0.0.1:18309 udp multipath; } }",
            &compile_prefix(),
        );
        assert!(error.contains("multipath") && error.contains("incompatible with \"udp\""));
    }
}

#[test]
fn rejects_http2_listen_option_in_stream_context() {
    let error = compile_error(
        "events {} stream { server { listen 127.0.0.1:18309 http2; } }",
        &compile_prefix(),
    );
    assert!(error.contains("invalid parameter \"http2\""));
}

fn compile_prefix() -> std::path::PathBuf {
    let prefix = temp_prefix();
    fs::create_dir_all(&prefix).unwrap();
    prefix
}

fn compile_error(source: &str, prefix: &std::path::Path) -> String {
    let config_path = prefix.join("nginx.conf");
    fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.to_path_buf())
        .load(&config_path)
        .unwrap();
    compile(document, prefix.to_path_buf(), config_path)
        .expect_err("configuration should fail")
        .to_string()
}
