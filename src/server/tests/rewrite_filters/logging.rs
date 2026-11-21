use super::*;

#[tokio::test]
async fn applies_rewrite_log_and_uninitialized_variable_warning_at_runtime() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            error_log logs/rewrite.log notice;
            rewrite_log on;
            uninitialized_variable_warn off;
            server {
                listen 127.0.0.1:18391;
                uninitialized_variable_warn on;
                location /logged/ {
                    rewrite ^/logged/(.*)$ /target/$1 last;
                }
                location /target/ {
                    return 200 "$uri";
                }
                location /iflog {
                    if ($arg_mode ~ "^on$") {
                        uninitialized_variable_warn off;
                        set $matched yes;
                        set $ignored $missing_inside_if;
                        set $missing_inside_if later;
                    }
                    return 200 "$matched";
                }
                location /quiet/ {
                    rewrite_log off;
                    uninitialized_variable_warn off;
                    set $copy $missing_quiet;
                    set $missing_quiet later;
                    rewrite ^/quiet/(.*)$ /target/$1 last;
                }
                location /loclog/ {
                    error_log logs/location-rewrite.log notice;
                    rewrite ^/loclog/(.*)$ /target/$1 last;
                }
                location /debuglog/ {
                    error_log logs/debug-rewrite.log debug;
                    rewrite_log off;
                    rewrite ^/debuglog/(.*)$ /target/$1 last;
                }
                location /warn {
                    rewrite ^/never$ /target last;
                    set $copy $missing_warn;
                    set $missing_warn later;
                    if ($arg_never = yes) {
                        set $late_for_header value;
                    }
                    add_header X-Late $late_for_header always;
                    return 204;
                }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let server = &config.http.as_ref().expect("http config").servers[0];
    assert!(server.context.rewrite_log);
    assert!(server.context.uninitialized_variable_warn);
    let quiet = match_location(&server.locations, "/quiet/item").expect("quiet location");
    assert!(!quiet.context.rewrite_log);
    assert!(!quiet.context.uninitialized_variable_warn);
    let iflog = match_location(&server.locations, "/iflog").expect("if location");
    assert!(iflog.context.rewrite_log);
    assert!(!iflog.context.conditionals[0].uninitialized_variable_warn);

    let state = test_state(&config);
    let group = build_listener_groups(&config)[0].clone();
    let remote = SocketAddr::from(([127, 0, 0, 1], 45678));
    let connection =
        ConnectionAddresses::direct(ListenAddress::Inet(remote), group.address.clone());
    for target in [
        "/logged/item",
        "/iflog?mode=on",
        "/quiet/item",
        "/loclog/item",
        "/debuglog/item",
        "/warn",
    ] {
        let request = Request {
            method: "GET".into(),
            target: target.into(),
            version: "HTTP/1.1".into(),
            headers: vec![Header {
                name: "Host".into(),
                value: "localhost".into(),
            }],
            body_stream: None,
            body: Vec::new(),
            remote_addr: remote.to_string(),
            local_addr: "127.0.0.1:18391".into(),
            keep_alive: false,
        };
        let snapshot = state.config_snapshot().await;
        let action = dispatch_http_request(
            snapshot,
            &group,
            &request,
            connection.clone(),
            ConnectionRequestState::new(std::time::Instant::now(), 0),
            None,
            &state,
        )
        .await
        .expect("HTTP dispatch");
        assert!(matches!(action, DispatchAction::Response(_)));
    }

    let log = std::fs::read_to_string(prefix.join("logs/rewrite.log")).unwrap();
    assert!(
        log.contains("\"^/logged/(.*)$\" matches \"/logged/item\""),
        "{log}"
    );
    assert!(
        log.contains("rewritten data: \"/target/item\", args: \"\""),
        "{log}"
    );
    assert!(
        log.contains("\"^/never$\" does not match \"/warn\""),
        "{log}"
    );
    assert!(log.contains("\"^on$\" matches \"on\""), "{log}");
    assert!(!log.contains("missing_inside_if"), "{log}");
    assert!(
        log.contains("using uninitialized \"missing_warn\" variable"),
        "{log}"
    );
    assert!(
        log.contains("using uninitialized \"late_for_header\" variable"),
        "{log}"
    );
    assert!(!log.contains("missing_quiet"), "{log}");
    assert!(!log.contains("\"/quiet/item\""), "{log}");
    assert!(!log.contains("\"/loclog/item\""), "{log}");
    let location_log = std::fs::read_to_string(prefix.join("logs/location-rewrite.log")).unwrap();
    assert!(
        location_log.contains("\"^/loclog/(.*)$\" matches \"/loclog/item\""),
        "{location_log}"
    );
    assert!(
        location_log.contains("rewritten data: \"/target/item\", args: \"\""),
        "{location_log}"
    );
    let debug_log = std::fs::read_to_string(prefix.join("logs/debug-rewrite.log")).unwrap();
    assert!(
        debug_log.contains("\"^/debuglog/(.*)$\" matches \"/debuglog/item\""),
        "{debug_log}"
    );
    assert!(
        debug_log.contains("rewritten data: \"/target/item\", args: \"\""),
        "{debug_log}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
