use super::support::*;
use super::*;

#[test]
fn compiles_and_overrides_valid_referers() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    server_name example.test;
                    valid_referers none blocked server_names good.test/path *.trusted.test;
                    location /inherit { return 200 "inherit\n"; }
                    location /replace {
                        valid_referers evil.test;
                        return 200 "replace\n";
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let inherited = server
        .locations
        .iter()
        .find(|location| {
            matches!(
                &location.matcher,
                LocationMatcher::Prefix(path) if path == "/inherit"
            )
        })
        .unwrap();
    let replaced = server
        .locations
        .iter()
        .find(|location| {
            matches!(
                &location.matcher,
                LocationMatcher::Prefix(path) if path == "/replace"
            )
        })
        .unwrap();
    assert!(server.context.valid_referers.no_referer);
    assert!(server.context.valid_referers.blocked_referer);
    assert!(server.context.valid_referers.server_names);
    assert_eq!(server.context.valid_referers.hosts.len(), 2);
    assert_eq!(inherited.context.valid_referers.hosts.len(), 2);
    assert!(!replaced.context.valid_referers.no_referer);
    assert_eq!(replaced.context.valid_referers.hosts.len(), 1);
    assert!(
        replaced.context.valid_referers.hosts[0]
            .host
            .matches("evil.test")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_valid_referers_wildcards() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    valid_referers foo*bar.test;
                    return 200 "ok\n";
                }
            }
        "#,
    );
    assert!(error.contains("invalid referer wildcard"));
}

#[test]
fn validates_none_and_blocked_after_merging_directives() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    valid_referers none;
                    valid_referers good.test;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );
    assert!(
        config.http.as_ref().unwrap().servers[0]
            .context
            .valid_referers
            .no_referer
    );
    let _ = std::fs::remove_dir_all(prefix);

    for directive in ["none", "blocked"] {
        let error = test_config_error(&format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18099;
                        valid_referers {directive};
                        return 200 "ok\n";
                    }}
                }}
            "#
        ));
        assert!(
            error.contains(
                "the \"none\" or \"blocked\" referers are specified in the \"valid_referers\" directive without any valid referer"
            ),
            "{error}"
        );
    }
}

#[test]
fn rejects_location_none_without_its_own_valid_referer() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    valid_referers good.test;
                    location / {
                        valid_referers none;
                        return 200 "ok\n";
                    }
                }
            }
        "#,
    );
    assert!(error.contains("without any valid referer"), "{error}");
}

#[tokio::test]
async fn enforces_valid_referers_and_invalid_referer_variable() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    server_name example.test;
                    valid_referers none blocked server_names good.test/path *.trusted.test
                                   ~^media[0-9]+\.test/;
                    location / {
                        if ($invalid_referer) { return 403 "blocked\n"; }
                        return 200 "valid\n";
                    }
                }
            }
        "#,
        &[],
    );

    for (referer, expected_status, expected_body) in [
        (None, 200, "valid\n"),
        (Some("https://example.test/page"), 200, "valid\n"),
        (Some("http://sub.trusted.test/file"), 200, "valid\n"),
        (Some("https://good.test/path/file"), 200, "valid\n"),
        (Some("https://good.test/other"), 403, "blocked\n"),
        (Some("https://media42.test/file"), 200, "valid\n"),
        (Some("https://evil.test/file"), 403, "blocked\n"),
        (Some("not-a-url"), 200, "valid\n"),
    ] {
        let response = request_bytes(&config, &referer_request(referer)).await;
        let (head, body) = split_http_response(&response);
        let status = head
            .split_whitespace()
            .nth(1)
            .and_then(|status| status.parse::<u16>().ok())
            .unwrap();
        assert_eq!(status, expected_status, "referer {referer:?}");
        assert_eq!(std::str::from_utf8(body).unwrap(), expected_body);
    }
    let _ = std::fs::remove_dir_all(prefix);
}

fn referer_request(referer: Option<&str>) -> String {
    let mut request = String::from("GET / HTTP/1.1\r\nHost: example.test\r\n");
    if let Some(referer) = referer {
        request.push_str("Referer: ");
        request.push_str(referer);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    request
}
