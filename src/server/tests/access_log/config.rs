use super::super::support::*;
use crate::config::AccessLogBufferConfig;
use std::time::Duration;

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
