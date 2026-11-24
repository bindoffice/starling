use super::support::*;
use crate::config::AccessLogBufferConfig;
use crate::logging::flush_access_logs;
use flate2::read::MultiGzDecoder;
use std::io::Read;
use std::time::Duration;

mod cache;
mod syslog;

#[tokio::test]
async fn logs_nginx_time_request_length_and_wire_bytes() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format metrics '$time_local|$request_length|$bytes_sent|$body_bytes_sent|$pipe';
                server {
                    listen 127.0.0.1:18198;
                    access_log logs/metrics.log metrics;
                    return 200 "OK";
                }
            }
        "#,
        &[],
    );

    let response = request_bytes(&config, "GET /metrics HTTP/1.0\nHost: localhost\n\n").await;
    let log = std::fs::read_to_string(prefix.join("logs/metrics.log")).unwrap();
    let fields: Vec<_> = log.trim_end().split('|').collect();
    assert_eq!(fields.len(), 5, "{log}");
    assert!(
        regex::Regex::new(r"^\d\d/[A-Z][a-z]{2}/\d{4}:\d\d:\d\d:\d\d [+-]\d{4}$")
            .unwrap()
            .is_match(fields[0]),
        "{log}"
    );
    assert_eq!(fields[1], "39");
    assert_eq!(fields[2], response.len().to_string());
    assert_eq!(fields[3], "2");
    assert_eq!(fields[4], ".");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn writes_all_http_access_logs_with_independent_filters_and_overrides() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18188;
                    access_log logs/first.log status if=$arg_log;
                    access_log logs/second.log combined;
                    location /inherited {
                        return 200 "inherited\n";
                    }
                    location /replaced {
                        access_log logs/replaced.log status;
                        return 200 "replaced\n";
                    }
                    location /disabled {
                        access_log off;
                        return 200 "disabled\n";
                    }
                }
            }
        "#,
        &[],
    );

    for path in [
        "/inherited?log=1",
        "/inherited?log=0",
        "/replaced",
        "/disabled",
    ] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{path}: {response}"
        );
    }

    let first = std::fs::read_to_string(prefix.join("logs/first.log")).unwrap_or_default();
    assert_eq!(first, "200\n");

    let second = std::fs::read_to_string(prefix.join("logs/second.log")).unwrap_or_default();
    assert!(second.contains("GET /inherited?log=1 HTTP/1.1"), "{second}");
    assert!(second.contains("GET /inherited?log=0 HTTP/1.1"), "{second}");
    assert!(!second.contains("/replaced"), "{second}");
    assert!(!second.contains("/disabled"), "{second}");

    let replaced = std::fs::read_to_string(prefix.join("logs/replaced.log")).unwrap_or_default();
    assert_eq!(replaced, "200\n");

    let disabled = std::fs::read_to_string(prefix.join("logs/disabled.log"));
    assert!(!disabled.is_ok_and(|contents| !contents.is_empty()));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn conditional_access_log_override_is_replaced_by_a_later_matched_if() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18196;
                    access_log logs/base.log status;
                    location /if-log {
                        if ($arg_first = yes) {
                            access_log logs/first.log status;
                        }
                        if ($arg_second = yes) {
                        }
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );

    for path in ["/if-log?first=yes", "/if-log?second=yes"] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    }

    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/first.log")).unwrap(),
        "200\n"
    );
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/base.log")).unwrap(),
        "200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_log_format_escape_mode_to_http_access_log_variables() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format escaped escape=json '$http_x_escape';
                server {
                    listen 127.0.0.1:18187;
                    access_log logs/escaped.log escaped;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nX-Escape: quote\"backslash\\\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/escaped.log")).unwrap(),
        "quote\\\"backslash\\\\\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn writes_all_inherited_stream_access_logs_with_independent_filters() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                log_format filter '$server_name';
                log_format protocol '$protocol';
                access_log logs/first.log filter if=$server_name;
                access_log logs/second.log protocol;
                server {
                    listen 127.0.0.1:13319;
                    server_name multi.test;
                    return "ok";
                }
            }
        "#,
        &[],
    );

    assert_eq!(stream_response(&config, b"").await, b"ok");
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/first.log")).unwrap(),
        "multi.test\n"
    );
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/second.log")).unwrap(),
        "TCP\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn expands_variables_in_http_access_log_paths() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18189;
                    access_log logs/$arg_name.log status;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET /?name=tenant HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/tenant.log")).unwrap(),
        "200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn expands_variables_in_stream_access_log_paths() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                log_format session '$server_name';
                access_log logs/$server_name-$protocol.log session;
                server {
                    listen 127.0.0.1:13320;
                    server_name dynamic.test;
                    return "ok";
                }
            }
        "#,
        &[],
    );

    assert_eq!(stream_response(&config, b"").await, b"ok");
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/dynamic.test-TCP.log")).unwrap(),
        "dynamic.test\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_unknown_and_duplicate_log_formats() {
    for (source, expected) in [
        (
            r#"
                events {}
                http {
                    server {
                        access_log logs/unknown.log missing;
                    }
                }
            "#,
            "unknown log format \"missing\"",
        ),
        (
            r#"
                events {}
                http {
                    log_format custom '$status';
                    log_format custom '$request';
                }
            "#,
            "duplicate \"log_format\" name \"custom\"",
        ),
        (
            r#"
                events {}
                http {
                    log_format combined '$status';
                }
            "#,
            "duplicate \"log_format\" name \"combined\"",
        ),
        (
            r#"
                events {}
                http {
                    server {
                        location / {
                            if ($arg_log = yes) {
                                access_log logs/conditional.log missing;
                            }
                        }
                    }
                }
            "#,
            "unknown log format \"missing\"",
        ),
    ] {
        let error = test_config_error(source);
        assert!(error.contains(expected), "unexpected error: {error}");
    }
}

#[test]
fn compiles_access_log_buffer_parameters() {
    let (_prefix, config) = test_config(
        r#"
            events {}
            http {
                server {
                    access_log logs/buffered.log combined buffer=4k flush=250ms gzip=5;
                }
            }
        "#,
        &[],
    );
    let target = &config.http.as_ref().unwrap().servers[0]
        .context
        .access_log
        .logs[0];
    assert_eq!(
        target.buffer,
        Some(AccessLogBufferConfig {
            size: 4096,
            flush: Some(Duration::from_millis(250)),
            gzip_level: Some(5),
        })
    );
}

#[test]
fn rejects_invalid_access_log_buffer_parameters() {
    for (parameters, expected) in [
        ("flush=10ms", "no buffer is defined"),
        ("buffer=0", "invalid buffer size"),
        ("buffer=4k flush=0", "invalid flush time"),
        ("buffer=4k gzip=0", "invalid compression level"),
        ("buffer=4k gzip=10", "invalid compression level"),
        ("buffer=4k gzip=invalid", "invalid compression level"),
    ] {
        let source = format!(
            r#"
                events {{}}
                http {{
                    server {{
                        access_log logs/buffered.log combined {parameters};
                    }}
                }}
            "#
        );
        let error = test_config_error(&source);
        assert!(
            error.contains(expected),
            "unexpected error for {parameters}: {error}"
        );
    }

    let dynamic = test_config_error(
        r#"
            events {}
            http {
                server {
                    access_log logs/$arg_name.log combined buffer=4k;
                }
            }
        "#,
    );
    assert!(
        dynamic.contains("buffered logs cannot have variables in name"),
        "{dynamic}"
    );

    let conflict = test_config_error(
        r#"
            events {}
            http {
                server {
                    access_log logs/shared.log combined buffer=1k;
                    access_log logs/shared.log combined buffer=2k;
                }
            }
        "#,
    );
    assert!(
        conflict.contains("already defined with conflicting parameters"),
        "{conflict}"
    );
}

#[tokio::test]
async fn flushes_buffered_http_access_logs_on_timer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18190;
                    access_log logs/buffered.log status buffer=4k flush=20ms;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/buffered.log")).unwrap(),
        "200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn gzip_compresses_flushed_http_access_logs() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18191;
                    access_log logs/buffered.log status buffer=4k gzip=6;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    flush_access_logs();
    let compressed = std::fs::read(prefix.join("logs/buffered.log")).unwrap();
    let mut decoder = MultiGzDecoder::new(compressed.as_slice());
    let mut contents = String::new();
    decoder.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "200\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn flushes_buffered_stream_access_logs_on_timer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                log_format session '$server_name';
                access_log logs/buffered.log session buffer=4k flush=20ms;
                server {
                    listen 127.0.0.1:13321;
                    server_name buffered.test;
                    return "ok";
                }
            }
        "#,
        &[],
    );

    assert_eq!(stream_response(&config, b"").await, b"ok");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/buffered.log")).unwrap(),
        "buffered.test\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
