use super::support::*;
use super::*;

#[test]
fn reload_signature_tracks_sockets_not_server_membership() {
    let (first_prefix, first) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18088;
                        server_name first.test;
                        location / { return 200 "first\n"; }
                    }
                }
            "#,
        &[],
    );
    let (second_prefix, second) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18088;
                        server_name first.test;
                        location / { return 200 "first\n"; }
                    }
                    server {
                        listen 127.0.0.1:18088;
                        server_name second.test;
                        location / { return 200 "second\n"; }
                    }
                }
            "#,
        &[],
    );
    assert_eq!(listener_signature(&first), listener_signature(&second));
    let _ = std::fs::remove_dir_all(first_prefix);
    let _ = std::fs::remove_dir_all(second_prefix);
}

#[test]
fn orders_upstream_retry_candidates_and_prefers_backups_last() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    upstream backend {
                        server 127.0.0.1:19001 weight=2;
                        server 127.0.0.1:19002;
                        server 127.0.0.1:19003 backup;
                        server 127.0.0.1:19004 down;
                    }
                    server {
                        listen 127.0.0.1:18094;
                        location / { proxy_pass http://backend; }
                    }
                }
            "#,
        &[],
    );
    let upstream = config
        .http
        .as_ref()
        .unwrap()
        .upstreams
        .get("backend")
        .unwrap();
    let first: Vec<_> = upstream
        .select_candidates()
        .into_iter()
        .map(|server| server.address.as_str())
        .collect();
    assert_eq!(
        first,
        vec!["127.0.0.1:19001", "127.0.0.1:19002", "127.0.0.1:19003"]
    );
    let second: Vec<_> = upstream
        .select_candidates()
        .into_iter()
        .map(|server| server.address.as_str())
        .collect();
    assert_eq!(
        second,
        vec!["127.0.0.1:19001", "127.0.0.1:19002", "127.0.0.1:19003"]
    );
    let third: Vec<_> = upstream
        .select_candidates()
        .into_iter()
        .map(|server| server.address.as_str())
        .collect();
    assert_eq!(
        third,
        vec!["127.0.0.1:19002", "127.0.0.1:19001", "127.0.0.1:19003"]
    );
    assert!(!first.iter().any(|address| address.ends_with("19004")));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn parses_upstream_load_balancing_methods() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    upstream least {
                        least_conn;
                        server 127.0.0.1:19101;
                    }
                    upstream ip {
                        ip_hash;
                        server 127.0.0.1:19102;
                    }
                    upstream randomized {
                        random two least_conn;
                        server 127.0.0.1:19103;
                    }
                    upstream hashed {
                        hash $request_uri;
                        server 127.0.0.1:19104;
                    }
                    upstream consistent {
                        hash $request_uri consistent;
                        server 127.0.0.1:19105;
                    }
                    upstream least_time_header {
                        least_time header inflight;
                        server 127.0.0.1:19106;
                    }
                    upstream least_time_last_byte {
                        least_time last_byte;
                        server 127.0.0.1:19107;
                    }
                    server {
                        listen 127.0.0.1:18095;
                        location / { return 200 "ok\n"; }
                    }
                }
            "#,
        &[],
    );
    let upstreams = &config.http.as_ref().unwrap().upstreams;
    assert_eq!(upstreams["least"].method, UpstreamMethod::LeastConn);
    assert_eq!(upstreams["ip"].method, UpstreamMethod::IpHash);
    assert_eq!(
        upstreams["randomized"].method,
        UpstreamMethod::Random { two: true }
    );
    assert_eq!(
        upstreams["hashed"].method,
        UpstreamMethod::Hash { consistent: false }
    );
    assert_eq!(
        upstreams["hashed"].hash_key.as_deref(),
        Some("$request_uri")
    );
    assert_eq!(
        upstreams["consistent"].method,
        UpstreamMethod::Hash { consistent: true }
    );
    assert_eq!(
        upstreams["least_time_header"].method,
        UpstreamMethod::LeastTime(LeastTimeConfig {
            mode: LeastTimeMode::Header,
            inflight: true,
        })
    );
    assert_eq!(
        upstreams["least_time_last_byte"].method,
        UpstreamMethod::LeastTime(LeastTimeConfig {
            mode: LeastTimeMode::LastByte,
            inflight: false,
        })
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_upstream_load_balancing_directives() {
    assert!(
        test_config_error(&upstream_config("least_conn two;"))
            .contains("invalid number of arguments")
    );
    assert!(
        test_config_error(&upstream_config("random one;")).contains("invalid parameter \"one\"")
    );
    assert!(
        test_config_error(&upstream_config("hash $request_uri random;"))
            .contains("invalid parameter \"random\"")
    );
    assert!(
        test_config_error(&upstream_config("least_time;")).contains("invalid number of arguments")
    );
    assert!(
        test_config_error(&upstream_config("least_time connect;"))
            .contains("invalid parameter \"connect\"")
    );
    assert!(
        test_config_error(&upstream_config("least_time header bogus;"))
            .contains("invalid parameter \"bogus\"")
    );
    assert!(
        test_config_error(&upstream_config("least_conn; ip_hash;"))
            .contains("load balancing method redefined")
    );
    assert!(
        test_config_error(&upstream_config("server 127.0.0.1:19001 weight=0;"))
            .contains("invalid weight")
    );
    assert!(
        test_config_error(&upstream_config("server 127.0.0.1:19001 unknown=1;"))
            .contains("invalid parameter \"unknown=1\"")
    );
    assert!(
        test_config_error(&upstream_config("server 127.0.0.1:19001 max_fails=bad;"))
            .contains("invalid value \"bad\"")
    );
}

#[test]
fn selects_ip_hash_and_hash_targets_deterministically() {
    let (prefix, config) = test_config(
        &upstream_config("hash $request_uri; server 127.0.0.1:19111; server 127.0.0.1:19112;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    let selected = upstream
        .select_with(None, Some("/stable?key=1"))
        .unwrap()
        .address
        .clone();
    assert_eq!(selected, "127.0.0.1:19112");
    for _ in 0..10 {
        assert_eq!(
            upstream
                .select_with(None, Some("/stable?key=1"))
                .unwrap()
                .address,
            selected
        );
    }
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        &upstream_config("ip_hash; server 127.0.0.1:19113; server 127.0.0.1:19114;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    for peer in ["192.0.2.10", "2001:db8::10"] {
        let peer = peer.parse().unwrap();
        let selected = upstream
            .select_with(Some(peer), None)
            .unwrap()
            .address
            .clone();
        assert_eq!(selected, "127.0.0.1:19114");
        for _ in 0..10 {
            assert_eq!(
                upstream.select_with(Some(peer), None).unwrap().address,
                selected
            );
        }
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn least_conn_prefers_idle_peers_and_releases_guards() {
    let (prefix, config) = test_config(
        &upstream_config("least_conn; server 127.0.0.1:19121; server 127.0.0.1:19122;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    let busy = &upstream.servers[0];
    let guard = busy.try_acquire().unwrap();
    assert_eq!(busy.active_connections(), 1);
    for _ in 0..5 {
        assert_eq!(
            upstream.select_with(None, None).unwrap().address,
            "127.0.0.1:19122"
        );
    }
    drop(guard);
    assert_eq!(busy.active_connections(), 0);
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn least_time_uses_header_and_last_byte_metrics() {
    let (prefix, config) = test_config(
        &upstream_config("least_time header; server 127.0.0.1:19123; server 127.0.0.1:19124;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    for (index, header, response) in [(0, 500, 500), (1, 20, 1_000)] {
        let mut guard = upstream.servers[index].try_acquire().unwrap();
        guard.record_timing(UpstreamAttemptTiming {
            connect: Duration::from_millis(5),
            first_byte: Some(Duration::from_millis(header)),
            header: Duration::from_millis(header),
            response: Duration::from_millis(response),
        });
    }
    assert_eq!(
        upstream.select_with(None, None).unwrap().address,
        "127.0.0.1:19124"
    );
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        &upstream_config("least_time last_byte; server 127.0.0.1:19125; server 127.0.0.1:19126;"),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    for (index, header, response) in [(0, 500, 500), (1, 20, 1_000)] {
        let mut guard = upstream.servers[index].try_acquire().unwrap();
        guard.record_timing(UpstreamAttemptTiming {
            connect: Duration::from_millis(5),
            first_byte: Some(Duration::from_millis(header)),
            header: Duration::from_millis(header),
            response: Duration::from_millis(response),
        });
    }
    assert_eq!(
        upstream.select_with(None, None).unwrap().address,
        "127.0.0.1:19125"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn parses_upstream_server_limits_and_health_settings() {
    let (prefix, config) = test_config(
        &upstream_config(
            "server 127.0.0.1:19131 weight=3 max_conns=4 max_fails=2 fail_timeout=7s;",
        ),
        &[],
    );
    let server = &config.http.as_ref().unwrap().upstreams["backend"].servers[0];
    assert_eq!(server.weight, 3);
    assert_eq!(server.max_conns, 4);
    assert_eq!(server.max_fails, 2);
    assert_eq!(server.fail_timeout, Duration::from_secs(7));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn enforces_max_conns_and_skips_recently_failed_peers() {
    let (prefix, config) = test_config(
        &upstream_config(
            "server 127.0.0.1:19141 max_conns=1 max_fails=1 fail_timeout=10s; \
                 server 127.0.0.1:19142 max_fails=1 fail_timeout=10s; \
                 server 127.0.0.1:19143 backup;",
        ),
        &[],
    );
    let upstream = &config.http.as_ref().unwrap().upstreams["backend"];
    let first = &upstream.servers[0];
    let held = first.try_acquire().unwrap();
    assert!(first.try_acquire().is_none());
    assert_eq!(first.active_connections(), 1);
    drop(held);
    assert_eq!(first.active_connections(), 0);

    let mut failed = first.try_acquire().unwrap();
    failed.mark_failed();
    drop(failed);
    let candidates: Vec<_> = upstream
        .select_candidates()
        .into_iter()
        .map(|server| server.address.as_str())
        .collect();
    assert_eq!(candidates, vec!["127.0.0.1:19142", "127.0.0.1:19143"]);
    let _ = std::fs::remove_dir_all(prefix);
}
