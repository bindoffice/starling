use super::*;

#[test]
fn inherits_and_overrides_stream_connection_limits() {
    let (prefix, config) = test_config(
        &stream_config(
            r#"
                limit_conn_zone $binary_remote_addr zone=perip:10m;
                limit_conn_zone $server_name zone=perserver:10m;
                limit_conn perip 10;
                limit_conn perserver 20;
                limit_conn_dry_run on;
                limit_conn_log_level warn;
                server {
                    listen 127.0.0.1:13330;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13331;
                    limit_conn perip 2;
                    limit_conn_dry_run off;
                    limit_conn_log_level info;
                    return overridden;
                }
            "#,
        ),
        &[],
    );
    let stream = config.stream.as_ref().unwrap();
    assert_eq!(stream.limit_conn_zones.len(), 2);
    assert_eq!(stream.servers[0].limit_conns.len(), 2);
    assert_eq!(stream.servers[0].limit_conns[0].connections, 10);
    assert_eq!(stream.servers[0].limit_conns[1].connections, 20);
    assert!(stream.servers[0].limit_conn_dry_run);
    assert_eq!(stream.servers[0].limit_conn_log_level, LogLevel::Warn);
    assert_eq!(stream.servers[1].limit_conns.len(), 1);
    assert_eq!(stream.servers[1].limit_conns[0].zone, "perip");
    assert_eq!(stream.servers[1].limit_conns[0].connections, 2);
    assert!(!stream.servers[1].limit_conn_dry_run);
    assert_eq!(stream.servers[1].limit_conn_log_level, LogLevel::Info);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_connection_limit_directives() {
    let malformed_zone = test_config_error(&stream_config(
        r#"
            limit_conn_zone $remote_addr zone=perip;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(malformed_zone.contains("invalid zone size \"perip\""));

    let duplicate_zone = test_config_error(&stream_config(
        r#"
            limit_conn_zone $remote_addr zone=perip:10m;
            limit_conn_zone $server_name zone=perip:10m;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(duplicate_zone.contains("duplicate limit_conn_zone"));

    let unknown_zone = test_config_error(&stream_config(
        r#"
            limit_conn missing 1;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(unknown_zone.contains("unknown limit_conn_zone \"missing\""));

    let zero = test_config_error(&stream_config(
        r#"
            limit_conn_zone $remote_addr zone=perip:10m;
            limit_conn perip 0;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(zero.contains("invalid connection limit \"0\""));

    let too_large = test_config_error(&stream_config(
        r#"
            limit_conn_zone $remote_addr zone=perip:10m;
            limit_conn perip 65536;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(too_large.contains("connection limit must be less 65536"));

    let invalid_log_level = test_config_error(&stream_config(
        r#"
            limit_conn_log_level debug;
            server { listen 127.0.0.1:13332; return ok; }
        "#,
    ));
    assert!(invalid_log_level.contains("invalid value \"debug\""));
}

#[test]
fn rejects_stream_limit_req_directives() {
    let zone = test_config_error(&stream_config(
        r#"
            limit_req_zone $binary_remote_addr zone=api:10m rate=1r/s;
            server { listen 127.0.0.1:13333; return ok; }
        "#,
    ));
    assert!(zone.contains("unknown directive \"limit_req_zone\" in stream context"));

    let limit = test_config_error(&stream_config(
        r#"
            server {
                listen 127.0.0.1:13333;
                limit_req zone=api;
                return ok;
            }
        "#,
    ));
    assert!(limit.contains("unknown directive \"limit_req\" in stream server context"));
}
