use std::time::{SystemTime, UNIX_EPOCH};

use super::support::*;
use super::*;

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn timestamp(value: &str) -> i64 {
    httpdate::parse_http_date(value)
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn get(config: &Arc<RuntimeConfig>, path: &str) -> String {
    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    request_body(config, &request).await
}

#[tokio::test]
async fn applies_expires_fixed_relative_daily_and_variable_values() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18164;
                        location = /epoch { expires epoch; return 200 "ok\n"; }
                        location = /max { expires max; return 200 "ok\n"; }
                        location = /hour { expires 1h; return 200 "ok\n"; }
                        location = /compound { expires 1h30m; return 200 "ok\n"; }
                        location = /negative { expires -1h; return 200 "ok\n"; }
                        location = /daily { expires @23h59m59s; return 200 "ok\n"; }
                        location = /dynamic { expires $arg_expires; return 200 "ok\n"; }
                        location = /off { expires off; return 200 "ok\n"; }
                        location = /missing {
                            expires max;
                            return 404 "missing\n";
                        }
                        location = /conditional {
                            if ($arg_cache) { expires max; }
                            return 200 "ok\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let epoch = get(&config, "/epoch").await;
    assert!(epoch.contains("Expires: Thu, 01 Jan 1970 00:00:01 GMT\r\n"));
    assert!(epoch.contains("Cache-Control: no-cache\r\n"));

    let max = get(&config, "/max").await;
    assert!(max.contains("Expires: Thu, 31 Dec 2037 23:55:55 GMT\r\n"));
    assert!(max.contains("Cache-Control: max-age=315360000\r\n"));

    for (path, expected_age) in [("/hour", 3600), ("/compound", 5400)] {
        let response = get(&config, path).await;
        let (headers, _) = split_http_response(response.as_bytes());
        let expected = format!("max-age={expected_age}");
        assert_eq!(
            header_value(headers, "Cache-Control"),
            Some(expected.as_str())
        );
        let expires = timestamp(header_value(headers, "Expires").unwrap());
        assert!((expires - now() - expected_age).abs() <= 5, "{headers}");
    }

    let negative = get(&config, "/negative").await;
    let (headers, _) = split_http_response(negative.as_bytes());
    assert_eq!(header_value(headers, "Cache-Control"), Some("no-cache"));
    assert!((timestamp(header_value(headers, "Expires").unwrap()) - now() + 3600).abs() <= 5);

    let daily = get(&config, "/daily").await;
    let (headers, _) = split_http_response(daily.as_bytes());
    let max_age = header_value(headers, "Cache-Control")
        .unwrap()
        .strip_prefix("max-age=")
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!((0..=86_400).contains(&max_age), "{headers}");
    assert!((timestamp(header_value(headers, "Expires").unwrap()) - now()).abs() <= 86_400);

    let dynamic = get(&config, "/dynamic?expires=2h").await;
    assert!(dynamic.contains("Cache-Control: max-age=7200\r\n"));
    let dynamic_epoch = get(&config, "/dynamic?expires=epoch").await;
    assert!(dynamic_epoch.contains("Cache-Control: no-cache\r\n"));
    let dynamic_off = get(&config, "/dynamic?expires=off").await;
    assert!(!dynamic_off.contains("Expires:"));
    let dynamic_invalid = get(&config, "/dynamic?expires=invalid").await;
    assert!(!dynamic_invalid.contains("Expires:"));
    assert!(!dynamic_invalid.contains("Cache-Control:"));

    let off = get(&config, "/off").await;
    assert!(!off.contains("Expires:"));
    assert!(!off.contains("Cache-Control:"));

    let missing = get(&config, "/missing").await;
    assert!(missing.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(!missing.contains("Expires:"));

    let conditional = get(&config, "/conditional?cache=1").await;
    assert!(conditional.contains("Cache-Control: max-age=315360000\r\n"));
    let unconditional = get(&config, "/conditional").await;
    assert!(!unconditional.contains("Expires:"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn later_matched_if_restores_inherited_expires_loc_conf() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18166;
                        location = /if-loc-conf {
                            if ($arg_first = yes) {
                                expires max;
                            }
                            if ($arg_second = yes) {
                            }
                            return 200 "ok\n";
                        }
                        location = /if-override {
                            expires 1h;
                            if ($arg_first = yes) {
                                expires max;
                            }
                            return 200 "ok\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let first = get(&config, "/if-loc-conf?first=yes").await;
    assert!(
        first.contains("Cache-Control: max-age=315360000\r\n"),
        "{first}"
    );

    let overwritten = get(&config, "/if-loc-conf?first=yes&second=yes").await;
    assert!(!overwritten.contains("Expires:"), "{overwritten}");
    assert!(!overwritten.contains("Cache-Control:"), "{overwritten}");

    let second = get(&config, "/if-loc-conf?second=yes").await;
    assert!(!second.contains("Expires:"), "{second}");
    assert!(!second.contains("Cache-Control:"), "{second}");

    let inherited = get(&config, "/if-override").await;
    assert!(
        inherited.contains("Cache-Control: max-age=3600\r\n"),
        "{inherited}"
    );
    let overridden = get(&config, "/if-override?first=yes").await;
    assert!(
        overridden.contains("Cache-Control: max-age=315360000\r\n"),
        "{overridden}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_modified_expires_from_last_modified() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18165;
                        root html;
                        location /future/ { expires modified 1h; }
                        location /past/ { expires modified -1h; }
                        location /dynamic/ { expires modified $arg_expires; }
                    }
                }
            "#,
        &[
            ("html/future/index.html", "future\n"),
            ("html/past/index.html", "past\n"),
            ("html/dynamic/index.html", "dynamic\n"),
        ],
    );

    let future = request_body(
        &config,
        "GET /future/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(future.as_bytes());
    let last_modified = timestamp(header_value(headers, "Last-Modified").unwrap());
    let expires = timestamp(header_value(headers, "Expires").unwrap());
    assert_eq!(expires, last_modified + 3600);
    let max_age = header_value(headers, "Cache-Control")
        .unwrap()
        .strip_prefix("max-age=")
        .unwrap()
        .parse::<i64>()
        .unwrap();
    assert!((3500..=3600).contains(&max_age), "{headers}");

    let past = request_body(
        &config,
        "GET /past/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(past.as_bytes());
    assert_eq!(header_value(headers, "Cache-Control"), Some("no-cache"));
    let last_modified = timestamp(header_value(headers, "Last-Modified").unwrap());
    let expires = timestamp(header_value(headers, "Expires").unwrap());
    assert_eq!(expires, last_modified - 3600);

    let dynamic = request_body(
        &config,
        "GET /dynamic/?expires=+1h HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(dynamic.as_bytes());
    let last_modified = timestamp(header_value(headers, "Last-Modified").unwrap());
    let expires = timestamp(header_value(headers, "Expires").unwrap());
    assert_eq!(expires, last_modified + 3600);

    let daily_with_modified = request_body(
        &config,
        "GET /dynamic/?expires=@1h HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!daily_with_modified.contains("Expires:"));
    let _ = std::fs::remove_dir_all(prefix);
}
