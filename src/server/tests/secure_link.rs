use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use md5::{Digest, Md5};

use super::support::*;
use super::*;

#[test]
fn compiles_secure_link_inheritance_and_mode_switching() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                secure_link_secret inherited;
                server {
                    listen 127.0.0.1:18099;
                    location /inherited { return 200 "ok\n"; }
                    location /modern {
                        secure_link $arg_st,$arg_e;
                        secure_link_md5 "$uri secret";
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server.context.secure_link.secret.as_deref(),
        Some("inherited")
    );
    let inherited = server
        .locations
        .iter()
        .find(|location| matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/inherited"))
        .unwrap();
    assert_eq!(
        inherited.context.secure_link.secret.as_deref(),
        Some("inherited")
    );
    let modern = server
        .locations
        .iter()
        .find(|location| matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/modern"))
        .unwrap();
    assert!(modern.context.secure_link.secret.is_none());
    assert_eq!(
        modern.context.secure_link.variable.as_deref(),
        Some("$arg_st,$arg_e")
    );
    assert_eq!(
        modern.context.secure_link.md5.as_deref(),
        Some("$uri secret")
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_duplicate_and_mixed_secure_link_directives() {
    for body in [
        r#"
            secure_link $arg_st;
            secure_link $arg_other;
        "#,
        r#"
            secure_link_secret one;
            secure_link_secret two;
        "#,
        r#"
            secure_link_secret one;
            secure_link $arg_st;
            secure_link_md5 "$uri secret";
        "#,
    ] {
        let error = test_config_error(&format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18099;
                        location / {{{body} return 200 "ok\n"; }}
                    }}
                }}
            "#
        ));
        assert!(
            error.contains("duplicate") || error.contains("\"secure_link_secret\" cannot be mixed"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn validates_modern_secure_links_and_expiration() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    server_name example.test;
                    location /download/ {
                        secure_link $arg_st,$arg_e;
                        secure_link_md5 "$secure_link_expires$uri integration-secret";
                        if ($secure_link = "") { return 403 "invalid\n"; }
                        if ($secure_link = "0") { return 410 "expired\n"; }
                        return 200 "$secure_link\n";
                    }
                }
            }
        "#,
        &[],
    );

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let uri = "/download/file.txt";
    for (expires, expected_status, expected_body) in
        [(now + 3600, 200, "1\n"), (now - 1, 410, "expired\n")]
    {
        let signature = modern_signature(uri, &expires.to_string(), "integration-secret");
        let request = get_request(&format!("/download/file.txt?st={signature}&e={expires}"));
        let response = request_bytes(&config, &request).await;
        assert_response(&response, expected_status, expected_body);
    }

    let padded_expires = format!("000{}", now + 3600);
    let signature = modern_signature(uri, &padded_expires, "integration-secret");
    let request = get_request(&format!(
        "/download/file.txt?st={signature}&e={padded_expires}"
    ));
    let response = request_bytes(&config, &request).await;
    assert_response(&response, 200, "1\n");

    let request = get_request(&format!(
        "/download/file.txt?st={}&e={}",
        "A".repeat(22),
        now + 3600
    ));
    let response = request_bytes(&config, &request).await;
    assert_response(&response, 403, "invalid\n");

    let response = request_bytes(&config, &get_request("/download/file.txt")).await;
    assert_response(&response, 403, "invalid\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn validates_legacy_secure_link_secret_mode() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    server_name example.test;
                    location /files/ {
                        secure_link_secret legacy-secret;
                        if ($secure_link = "") { return 403 "invalid\n"; }
                        return 200 "$secure_link\n";
                    }
                }
            }
        "#,
        &[],
    );

    let path = "archive/file.bin";
    let signature = hex_md5(&format!("{path}legacy-secret"));
    let request = get_request(&format!("/files/{signature}/{path}"));
    let response = request_bytes(&config, &request).await;
    assert_response(&response, 200, "archive/file.bin\n");

    let request = get_request(&format!("/files/{}/{path}", "0".repeat(32)));
    let response = request_bytes(&config, &request).await;
    assert_response(&response, 403, "invalid\n");
    let _ = std::fs::remove_dir_all(prefix);
}

fn modern_signature(uri: &str, expires: &str, secret: &str) -> String {
    let input = format!("{expires}{uri} {secret}");
    URL_SAFE_NO_PAD.encode(Md5::digest(input.as_bytes()))
}

fn hex_md5(input: &str) -> String {
    Md5::digest(input.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn get_request(target: &str) -> String {
    format!("GET {target} HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n")
}

fn assert_response(response: &[u8], status: u16, body: &str) {
    let (head, response_body) = split_http_response(response);
    let actual_status = head
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .unwrap();
    assert_eq!(actual_status, status, "{head}");
    assert_eq!(std::str::from_utf8(response_body).unwrap(), body);
}
