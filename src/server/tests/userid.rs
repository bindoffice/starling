use super::support::*;
use super::*;

#[test]
fn compiles_userid_inheritance_flags_and_validation() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                userid on;
                userid_name http-uid;
                userid_domain example.test;
                userid_flags secure;
                server {
                    listen 127.0.0.1:18175;
                    userid v1;
                    userid_service 42;
                    userid_expires max;
                    location /inherit { }
                    location /override {
                        userid off;
                        userid_name local-uid;
                        userid_domain none;
                        userid_path /local;
                        userid_expires 1h;
                        userid_flags httponly samesite=lax;
                        userid_p3p none;
                        userid_mark X;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server.context.userid.enable,
        crate::config::UserIdEnable::V1
    );
    assert_eq!(server.context.userid.service, Some(42));
    assert_eq!(server.context.userid.name, "http-uid");
    assert_eq!(
        server.context.userid.domain.as_deref(),
        Some("example.test")
    );
    assert_eq!(
        server.context.userid.expires,
        crate::config::UserIdExpires::Max
    );
    assert!(server.context.userid.flags.secure);

    let inherited = server
        .locations
        .iter()
        .find(|location| {
            matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/inherit")
        })
        .unwrap();
    assert_eq!(
        inherited.context.userid.enable,
        crate::config::UserIdEnable::V1
    );
    assert_eq!(inherited.context.userid.name, "http-uid");
    assert!(inherited.context.userid.flags.secure);

    let overridden = server
        .locations
        .iter()
        .find(|location| {
            matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/override")
        })
        .unwrap();
    assert_eq!(
        overridden.context.userid.enable,
        crate::config::UserIdEnable::Off
    );
    assert_eq!(overridden.context.userid.name, "local-uid");
    assert!(overridden.context.userid.domain.is_none());
    assert_eq!(overridden.context.userid.path, "/local");
    assert_eq!(
        overridden.context.userid.expires,
        crate::config::UserIdExpires::Seconds(3600)
    );
    assert!(!overridden.context.userid.flags.secure);
    assert!(overridden.context.userid.flags.http_only);
    assert!(overridden.context.userid.flags.same_site_lax);
    assert!(overridden.context.userid.p3p.is_none());
    assert_eq!(overridden.context.userid.mark, Some(b'X'));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_duplicate_and_invalid_userid_values() {
    let duplicate = test_config_error(
        r#"
            events {}
            http {
                userid on;
                userid v1;
            }
        "#,
    );
    assert!(
        duplicate.contains("\"userid\" directive is duplicate"),
        "{duplicate}"
    );

    for value in ["invalid", "toolong"] {
        let error = test_config_error(&format!(
            r#"
                events {{}}
                http {{
                    server {{
                        listen 127.0.0.1:18175;
                        userid_mark {value};
                    }}
                }}
            "#
        ));
        assert!(error.contains("invalid value"), "{error}");
    }
}

#[tokio::test]
async fn issues_and_reuses_userid_cookies_with_variables() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18175;
                    userid on;
                    userid_name starling;
                    userid_domain example.test;
                    userid_path /app;
                    userid_expires max;
                    userid_flags httponly secure samesite=lax;
                    userid_p3p "policy";
                    location /vars { return 200 "$uid_got|$uid_set"; }
                    location /reset {
                        set $uid_reset 1;
                        return 200 "$uid_got|$uid_set";
                    }
                    location /log {
                        userid log;
                        return 200 "$uid_got|$uid_set";
                    }
                    location /marked {
                        userid_mark X;
                        return 200 "$uid_got|$uid_set";
                    }
                    location /v1 {
                        userid v1;
                        userid_service 42;
                        return 200 "$uid_set";
                    }
                }
            }
        "#,
        &[],
    );

    let first_response = request_bytes(&config, &get_request("/vars")).await;
    let (first_headers, first_body) = split_http_response(&first_response);
    let first_body = std::str::from_utf8(first_body).unwrap();
    let (got, first_set) = first_body.split_once('|').unwrap();
    assert!(got.is_empty(), "{got}");
    assert!(first_set.starts_with("starling="), "{first_set}");
    assert_eq!(first_set.len(), "starling=".len() + 32);
    let cookie = response_cookie(first_headers, "starling").unwrap();
    assert_eq!(cookie.len(), 24);
    assert!(first_headers.contains("Set-Cookie: starling="));
    assert!(first_headers.contains(
        "; expires=Thu, 31-Dec-37 23:55:55 GMT; domain=example.test; path=/app; secure; httponly; samesite=lax"
    ), "{first_headers}");
    assert!(first_headers.contains("P3P: policy\r\n"), "{first_headers}");

    let with_cookie = request_bytes(
        &config,
        &format!(
            "GET /vars HTTP/1.1\r\nHost: localhost\r\nCookie: starling={cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    let (headers, body) = split_http_response(&with_cookie);
    let body = std::str::from_utf8(body).unwrap();
    assert_eq!(body, format!("{first_set}|"));
    assert!(!headers.contains("Set-Cookie:"), "{headers}");

    let reset = request_bytes(
        &config,
        &format!(
            "GET /reset HTTP/1.1\r\nHost: localhost\r\nCookie: starling={cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    let (headers, body) = split_http_response(&reset);
    let body = std::str::from_utf8(body).unwrap();
    let (got, set) = body.split_once('|').unwrap();
    assert_eq!(got, first_set);
    assert_ne!(set, first_set);
    let reset_cookie = response_cookie(headers, "starling").unwrap();
    assert_ne!(reset_cookie, cookie);

    let log = request_bytes(
        &config,
        &format!(
            "GET /log HTTP/1.1\r\nHost: localhost\r\nCookie: starling={cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    let (headers, body) = split_http_response(&log);
    let body = std::str::from_utf8(body).unwrap();
    assert_eq!(body, format!("{first_set}|"));
    assert!(!headers.contains("Set-Cookie:"), "{headers}");

    let marked = request_bytes(
        &config,
        &format!(
            "GET /marked HTTP/1.1\r\nHost: localhost\r\nCookie: starling={cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    let (headers, body) = split_http_response(&marked);
    let body = std::str::from_utf8(body).unwrap();
    assert_eq!(body, format!("{first_set}|{first_set}"));
    let marked_cookie = response_cookie(headers, "starling").unwrap();
    assert_eq!(&marked_cookie[..22], &cookie[..22]);
    assert_eq!(&marked_cookie[22..], "X=");

    let marked_again = request_bytes(
        &config,
        &format!(
            "GET /marked HTTP/1.1\r\nHost: localhost\r\nCookie: starling={marked_cookie}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    let (headers, _) = split_http_response(&marked_again);
    assert!(!headers.contains("Set-Cookie:"), "{headers}");

    let v1 = request_bytes(&config, &get_request("/v1")).await;
    let (_, v1) = split_http_response(&v1);
    let v1 = std::str::from_utf8(v1).unwrap();
    let service = if cfg!(target_endian = "little") {
        "0000002A"
    } else {
        "2A000000"
    };
    assert!(v1.starts_with(&format!("starling={service}")), "{v1}");
    assert_eq!(v1.len(), "starling=".len() + 32);
    let _ = std::fs::remove_dir_all(prefix);
}

fn response_cookie<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let value = line.strip_prefix("Set-Cookie: ")?;
        let value = value.strip_prefix(name)?.strip_prefix('=')?;
        Some(value.split(';').next().unwrap_or_default())
    })
}

fn get_request(target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
}
