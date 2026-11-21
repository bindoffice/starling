use std::io::Write as _;

use super::support::*;
use super::*;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

fn header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

#[tokio::test]
async fn decompresses_gzip_static_unless_the_client_accepts_gzip() {
    let plain = b"repeated gunzip payload repeated gunzip payload\n";
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gunzip on;
                    gzip_static always;
                    gzip_vary on;
                    server {
                        listen 127.0.0.1:18161;
                        root html;
                        location /on/ { }
                        location /off/ { gunzip off; }
                        location /novary/ {
                            gunzip on;
                            gzip_vary off;
                        }
                    }
                }
            "#,
        &[
            ("html/on/site.txt", "plain fallback\n"),
            ("html/off/site.txt", "plain fallback\n"),
            ("html/novary/site.txt", "plain fallback\n"),
        ],
    );
    let compressed = gzip(plain);
    std::fs::write(prefix.join("html/on/site.txt.gz"), &compressed).unwrap();
    std::fs::write(prefix.join("html/off/site.txt.gz"), &compressed).unwrap();
    std::fs::write(prefix.join("html/novary/site.txt.gz"), &compressed).unwrap();

    let http = config.http.as_ref().unwrap();
    let server = &http.servers[0];
    assert!(server.context.gunzip, "gunzip directive was not inherited");
    assert!(
        server.locations.iter().any(|location| {
            matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/on/")
                && location.context.gunzip
        }),
        "gunzip directive was not inherited by location"
    );
    assert!(
        server.locations.iter().any(|location| {
            matches!(&location.matcher, LocationMatcher::Prefix(path) if path == "/off/")
                && !location.context.gunzip
        }),
        "location gunzip off did not override inheritance"
    );

    let decoded = request_bytes(
        &config,
        "GET /on/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&decoded);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body, plain);
    assert!(!headers.contains("Content-Encoding:"), "{headers}");
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");
    let content_length = plain.len().to_string();
    assert_eq!(
        header_value(headers, "Content-Length"),
        Some(content_length.as_str())
    );
    assert!(
        header_value(headers, "ETag").is_some_and(|value| value.starts_with("W/\"")),
        "{headers}"
    );
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");

    let preserved = request_bytes(
        &config,
        "GET /on/site.txt HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&preserved);
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");
    assert_eq!(body, compressed);

    let disabled = request_bytes(
        &config,
        "GET /off/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&disabled);
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert_eq!(body, compressed);

    let no_vary = request_bytes(
        &config,
        "GET /novary/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&no_vary);
    assert_eq!(body, plain);
    assert!(!headers.contains("Vary:"), "{headers}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_sub_filters_after_decompressing() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gunzip on;
                    gzip_static always;
                    server {
                        listen 127.0.0.1:18162;
                        root html;
                        location / {
                            sub_filter_types text/plain;
                            sub_filter "uncompressed" "filtered";
                        }
                    }
                }
            "#,
        &[("html/site.txt", "plain fallback\n")],
    );
    std::fs::write(
        prefix.join("html/site.txt.gz"),
        gzip(b"uncompressed payload\n"),
    )
    .unwrap();

    let response = request_bytes(
        &config,
        "GET /site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(!headers.contains("Content-Encoding:"), "{headers}");
    assert_eq!(body, b"filtered payload\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_invalid_gzip_payloads() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gunzip on;
                    gzip_static always;
                    server {
                        listen 127.0.0.1:18163;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[
            ("html/site.txt", "plain fallback\n"),
            ("html/site.txt.gz", "not a gzip stream"),
        ],
    );

    let response = request_bytes(
        &config,
        "GET /site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, _) = split_http_response(&response);
    assert!(
        headers.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{headers}"
    );
    assert!(!headers.contains("Content-Encoding:"), "{headers}");
    let _ = std::fs::remove_dir_all(prefix);
}
