use std::time::Duration;

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

fn shifted_date(value: &str, seconds: i64) -> String {
    let time = httpdate::parse_http_date(value).unwrap();
    let time = if seconds >= 0 {
        time + Duration::from_secs(seconds as u64)
    } else {
        time - Duration::from_secs(seconds.unsigned_abs())
    };
    httpdate::fmt_http_date(time)
}

async fn request(config: &Arc<RuntimeConfig>, path: &str, headers: &str) -> Vec<u8> {
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n{headers}Connection: close\r\n\r\n");
    request_bytes(config, &request).await
}

#[tokio::test]
async fn applies_conditional_request_precedence_and_if_modified_since_modes() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18166;
                        root html;
                        location /exact/ { }
                        location /before/ { if_modified_since before; }
                        location /off/ { if_modified_since off; }
                    }
                }
            "#,
        &[
            ("html/exact/site.txt", "exact\n"),
            ("html/before/site.txt", "before\n"),
            ("html/off/site.txt", "off\n"),
        ],
    );

    let original = request(&config, "/exact/site.txt", "").await;
    let (headers, _) = split_http_response(&original);
    let etag = header_value(headers, "ETag").unwrap().to_string();
    let last_modified = header_value(headers, "Last-Modified").unwrap().to_string();
    let older = shifted_date(&last_modified, -60);
    let newer = shifted_date(&last_modified, 60);

    let equal = request(
        &config,
        "/exact/site.txt",
        &format!("If-Modified-Since: {last_modified}\r\n"),
    )
    .await;
    let (headers, body) = split_http_response(&equal);
    assert!(
        headers.starts_with("HTTP/1.1 304 Not Modified\r\n"),
        "{headers}"
    );
    assert!(body.is_empty());
    assert!(!headers.contains("Content-Type:"), "{headers}");
    assert!(!headers.contains("Content-Length:"), "{headers}");
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");
    assert!(headers.contains(&format!("ETag: {etag}\r\n")), "{headers}");
    assert!(
        headers.contains(&format!("Last-Modified: {last_modified}\r\n")),
        "{headers}"
    );

    let modified = request(
        &config,
        "/exact/site.txt",
        &format!("If-Modified-Since: {older}\r\n"),
    )
    .await;
    assert!(modified.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let newer_exact = request(
        &config,
        "/exact/site.txt",
        &format!("If-Modified-Since: {newer}\r\n"),
    )
    .await;
    assert!(newer_exact.starts_with(b"HTTP/1.1 200 OK\r\n"));

    let before = request(
        &config,
        "/before/site.txt",
        &format!("If-Modified-Since: {newer}\r\n"),
    )
    .await;
    assert!(before.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));

    let off = request(
        &config,
        "/off/site.txt",
        &format!("If-Modified-Since: {last_modified}\r\n"),
    )
    .await;
    assert!(off.starts_with(b"HTTP/1.1 200 OK\r\n"));

    let weak_match = request(
        &config,
        "/exact/site.txt",
        &format!("If-None-Match: W/{etag}\r\n"),
    )
    .await;
    assert!(weak_match.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    let any_match = request(&config, "/exact/site.txt", "If-None-Match: *\r\n").await;
    assert!(any_match.starts_with(b"HTTP/1.1 304 Not Modified\r\n"));
    let ims_priority = request(
        &config,
        "/exact/site.txt",
        &format!("If-Modified-Since: {older}\r\nIf-None-Match: {etag}\r\n"),
    )
    .await;
    assert!(ims_priority.starts_with(b"HTTP/1.1 200 OK\r\n"));

    let if_match = request(&config, "/exact/site.txt", &format!("If-Match: {etag}\r\n")).await;
    assert!(if_match.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let if_match_failed = request(&config, "/exact/site.txt", "If-Match: \"other\"\r\n").await;
    assert!(if_match_failed.starts_with(b"HTTP/1.1 412 Precondition Failed\r\n"));
    let weak_if_match = request(
        &config,
        "/exact/site.txt",
        &format!("If-Match: W/{etag}\r\n"),
    )
    .await;
    assert!(weak_if_match.starts_with(b"HTTP/1.1 412 Precondition Failed\r\n"));

    let unmodified = request(
        &config,
        "/exact/site.txt",
        &format!("If-Unmodified-Since: {last_modified}\r\n"),
    )
    .await;
    assert!(unmodified.starts_with(b"HTTP/1.1 200 OK\r\n"));
    let modified_after = request(
        &config,
        "/exact/site.txt",
        &format!("If-Unmodified-Since: {older}\r\n"),
    )
    .await;
    assert!(modified_after.starts_with(b"HTTP/1.1 412 Precondition Failed\r\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn routes_precondition_failures_through_error_pages() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18168;
                        root html;
                        error_page 412 /fallback.html;
                        location = /fallback.html { internal; }
                        location /override/ {
                            error_page 412 =200 /fallback.html;
                        }
                    }
                }
            "#,
        &[
            ("html/site.txt", "original\n"),
            ("html/override/site.txt", "original\n"),
            ("html/fallback.html", "precondition fallback\n"),
        ],
    );

    let preserved = request(&config, "/site.txt", "If-Match: \"other\"\r\n").await;
    let (headers, body) = split_http_response(&preserved);
    assert!(
        headers.starts_with("HTTP/1.1 412 Precondition Failed\r\n"),
        "{headers}"
    );
    assert_eq!(body, b"precondition fallback\n");

    let overwritten = request(&config, "/override/site.txt", "If-Match: \"other\"\r\n").await;
    let (headers, body) = split_http_response(&overwritten);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body, b"precondition fallback\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn removes_content_encoding_from_gzip_not_modified_responses() {
    use std::io::Write as _;

    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gzip_static always;
                    gzip_vary on;
                    gunzip on;
                    server {
                        listen 127.0.0.1:18167;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[("html/site.txt", "plain fallback\n")],
    );
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(b"compressed body\n").unwrap();
    std::fs::write(prefix.join("html/site.txt.gz"), encoder.finish().unwrap()).unwrap();

    let original = request(&config, "/site.txt", "Accept-Encoding: gzip\r\n").await;
    let (headers, _) = split_http_response(&original);
    let etag = header_value(headers, "ETag").unwrap().to_string();
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");

    let not_modified = request(
        &config,
        "/site.txt",
        &format!("Accept-Encoding: gzip\r\nIf-None-Match: {etag}\r\n"),
    )
    .await;
    let (headers, body) = split_http_response(&not_modified);
    assert!(
        headers.starts_with("HTTP/1.1 304 Not Modified\r\n"),
        "{headers}"
    );
    assert!(body.is_empty());
    assert!(!headers.contains("Content-Encoding:"), "{headers}");
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");
    let _ = std::fs::remove_dir_all(prefix);
}
