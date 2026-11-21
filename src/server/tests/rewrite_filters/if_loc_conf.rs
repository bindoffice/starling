use super::*;

#[test]
fn if_context_matches_nginx_directive_scope() {
    let access = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        location / {
                            if ($arg_block = yes) {
                                deny all;
                            }
                        }
                    }
                }
            "#,
    );
    assert!(
        access.contains("\"deny\" directive is not allowed here"),
        "{access}"
    );

    let server_loc_conf = test_config_error(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        if ($arg_cache = yes) {
                            expires max;
                        }
                    }
                }
            "#,
    );
    assert!(
        server_loc_conf.contains("\"expires\" directive is not allowed here"),
        "{server_loc_conf}"
    );
}

#[test]
fn if_conditions_require_parentheses_and_a_variable_left_operand() {
    for condition in ["$arg_debug", "(literal)", "(literal = value)"] {
        let error = test_config_error(&format!(
            r#"
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        location / {{
                            if {condition} {{
                                return 204;
                            }}
                        }}
                    }}
                }}
            "#
        ));
        assert!(error.contains("invalid condition"), "{condition}: {error}");
    }
}

#[test]
fn if_loc_conf_snapshots_include_gzip_and_charset() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        gzip on;
                        charset utf-8;
                        location /if-snapshot {
                            if ($arg_first = yes) {
                                gzip off;
                                charset koi8-r;
                            }
                            if ($arg_second = yes) {
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let conditionals = &config.http.as_ref().unwrap().servers[0].locations[0]
        .context
        .conditionals;
    assert!(!conditionals[0].gzip);
    assert_eq!(conditionals[0].charset.charset.as_deref(), Some("koi8-r"));
    assert!(conditionals[1].gzip);
    assert_eq!(conditionals[1].charset.charset.as_deref(), Some("utf-8"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn if_loc_conf_snapshots_include_core_directives() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        root html/base;
                        error_page 404 /base-error;
                        post_action /base-post;
                        location /if-core {
                            if ($arg_first = yes) {
                                root html/conditional;
                                sendfile off;
                                limit_rate 2k;
                                limit_rate_after 20k;
                                error_page 500 /conditional-error;
                                post_action /conditional-post;
                            }
                            if ($arg_second = yes) {
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let location = &config.http.as_ref().unwrap().servers[0].locations[0];
    let parent = &location.context;
    let conditionals = &parent.conditionals;

    assert_eq!(conditionals[0].root, prefix.join("html/conditional"));
    assert!(!conditionals[0].sendfile);
    assert_eq!(conditionals[0].limit_rate.as_deref(), Some("2k"));
    assert_eq!(conditionals[0].limit_rate_after.as_deref(), Some("20k"));
    assert_eq!(conditionals[0].error_pages[0].statuses, [500]);
    assert_eq!(
        conditionals[0].post_action.as_deref(),
        Some("/conditional-post")
    );

    assert_eq!(conditionals[1].root, parent.root);
    assert_eq!(conditionals[1].sendfile, parent.sendfile);
    assert_eq!(conditionals[1].limit_rate, parent.limit_rate);
    assert_eq!(conditionals[1].limit_rate_after, parent.limit_rate_after);
    assert_eq!(conditionals[1].error_pages[0].statuses, [404]);
    assert_eq!(conditionals[1].post_action, parent.post_action);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn matched_if_switches_and_restores_root() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19997;
                        root html/base;
                        location /files/ {
                            if ($arg_alt = yes) {
                                root html/conditional;
                            }
                            if ($arg_reset = yes) {
                            }
                        }
                    }
                }
            "#,
        &[
            ("html/base/files/item.txt", "base\n"),
            ("html/conditional/files/item.txt", "conditional\n"),
        ],
    );

    for (path, expected) in [
        ("/files/item.txt", "base\n"),
        ("/files/item.txt?alt=yes", "conditional\n"),
        ("/files/item.txt?alt=yes&reset=yes", "base\n"),
    ] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.ends_with(expected), "{path}: {response}");
    }
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn matched_if_switches_and_restores_error_page() {
    let (prefix, config) = test_config(
        r#"
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:19998;
                        root html;
                        error_page 404 /base-error;
                        location /files/ {
                            if ($arg_alt = yes) {
                                error_page 404 /conditional-error;
                            }
                            if ($arg_reset = yes) {
                            }
                            try_files $uri =404;
                        }
                        location = /base-error {
                            internal;
                            return 200 "base\n";
                        }
                        location = /conditional-error {
                            internal;
                            return 200 "conditional\n";
                        }
                    }
                }
            "#,
        &[],
    );

    for (path, expected) in [
        ("/files/missing", "base\n"),
        ("/files/missing?alt=yes", "conditional\n"),
        ("/files/missing?alt=yes&reset=yes", "base\n"),
    ] {
        let response = request_body(
            &config,
            &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.ends_with(expected), "{path}: {response}");
    }
    let _ = std::fs::remove_dir_all(prefix);
}
