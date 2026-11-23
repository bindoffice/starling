use super::*;

#[tokio::test]
async fn compresses_eligible_responses_with_gzip() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gzip on;
                    gzip_types text/plain;
                    gzip_min_length 10;
                    gzip_comp_level 6;
                    gzip_vary on;
                    server {
                        listen 127.0.0.1:18084;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[(
            "html/repeated.txt",
            "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz",
        )],
    );
    let response = request_bytes(
            &config,
            "GET /repeated.txt HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(
        config.http.as_ref().unwrap().servers[0].context.gzip,
        "gzip directive was not inherited"
    );
    assert!(
        config.http.as_ref().unwrap().servers[0].locations[0]
            .context
            .gzip,
        "gzip directive was not inherited by location"
    );
    assert_eq!(
        config.http.as_ref().unwrap().servers[0].locations[0]
            .context
            .gzip_types,
        vec!["text/plain"]
    );
    let test_request = Request {
        method: "GET".into(),
        target: "/repeated.txt".into(),
        version: "HTTP/1.1".into(),
        headers: vec![Header {
            name: "Accept-Encoding".into(),
            value: "gzip".into(),
        }],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1".into(),
        local_addr: "127.0.0.1:18084".into(),
        keep_alive: false,
    };
    let mut direct = Response::bytes(
        200,
        "text/plain",
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz".as_bytes(),
    );
    maybe_gzip(
        &mut direct,
        &config.http.as_ref().unwrap().servers[0].locations[0].context,
        &test_request,
        false,
    )
    .await;
    assert!(direct.header("Content-Encoding").is_some());
    assert!(accepts_gzip("gzip"));
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    let headers = std::str::from_utf8(&response[..header_end]).unwrap();
    assert!(
        headers.contains("Content-Encoding: gzip\r\n"),
        "response headers:\n{headers}"
    );
    assert!(headers.contains("Transfer-Encoding: chunked\r\n"));
    assert!(!headers.contains("Content-Length:"));
    assert!(headers.contains("Vary: Accept-Encoding\r\n"));
    let body = decode_chunked(&response[header_end + 4..]);
    let mut decoder = flate2::read::GzDecoder::new(body.as_slice());
    let mut decoded = String::new();
    std::io::Read::read_to_string(&mut decoder, &mut decoded).unwrap();
    assert_eq!(
        decoded,
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn decode_chunked(mut body: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&body[..line_end]).unwrap().trim(), 16)
                .unwrap();
        body = &body[line_end + 2..];
        if size == 0 {
            return decoded;
        }
        decoded.extend_from_slice(&body[..size]);
        body = &body[size + 2..];
    }
}

#[tokio::test]
async fn serves_precompressed_gzip_static_files() {
    use std::io::{Read as _, Write as _};

    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gzip_static on;
                    gzip_vary on;
                    server {
                        listen 127.0.0.1:18127;
                        root html;
                        location /always/ {
                            gzip_static always;
                        }
                        location /novary/ {
                            gzip_vary off;
                        }
                    }
                }
            "#,
        &[
            ("html/site.txt", "plain site\n"),
            ("html/always/site.txt", "always plain site\n"),
            ("html/novary/site.txt", "no vary site\n"),
        ],
    );
    let gzip = |data: &[u8]| {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    };
    std::fs::write(prefix.join("html/site.txt.gz"), gzip(b"compressed site\n")).unwrap();
    std::fs::write(
        prefix.join("html/always/site.txt.gz"),
        gzip(b"always compressed site\n"),
    )
    .unwrap();
    std::fs::write(
        prefix.join("html/novary/site.txt.gz"),
        gzip(b"no vary compressed site\n"),
    )
    .unwrap();
    std::fs::write(prefix.join("html/only.txt.gz"), gzip(b"gzip-only\n")).unwrap();

    let accepted = request_bytes(
            &config,
            "GET /site.txt HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await;
    let (headers, body) = split_http_response(&accepted);
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");
    let mut decoder = flate2::read::GzDecoder::new(body);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "compressed site\n");

    let declined = request_bytes(
        &config,
        "GET /site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&declined);
    assert!(!headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");
    assert_eq!(body, b"plain site\n");

    let no_vary = request_bytes(
        &config,
        "GET /novary/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&no_vary);
    assert!(!headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(!headers.contains("Vary: Accept-Encoding\r\n"), "{headers}");
    assert_eq!(body, b"no vary site\n");

    let always = request_bytes(
        &config,
        "GET /always/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&always);
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    let mut decoder = flate2::read::GzDecoder::new(body);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "always compressed site\n");

    let gzip_only = request_bytes(
            &config,
            "GET /only.txt HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
        )
        .await;
    let (headers, body) = split_http_response(&gzip_only);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    let mut decoder = flate2::read::GzDecoder::new(body);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded, "gzip-only\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn evaluates_gzip_proxied_response_conditions() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    gzip on;
                    gzip_types text/plain;
                    gzip_min_length 1;
                    gzip_proxied no-cache no_etag auth;
                    server {
                        listen 127.0.0.1:18138;
                        location / { }
                    }
                }
            "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let request = Request {
        method: "GET".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: vec![
            Header {
                name: "Accept-Encoding".into(),
                value: "gzip".into(),
            },
            Header {
                name: "Authorization".into(),
                value: "Basic dXNlcjpwYXNz".into(),
            },
        ],
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:1".into(),
        local_addr: "127.0.0.1:18138".into(),
        keep_alive: false,
    };
    let mut no_cache = Response::bytes(200, "text/plain", "cacheable body");
    no_cache.set_header("Cache-Control", "no-cache");
    maybe_gzip(&mut no_cache, context, &request, true).await;
    assert!(no_cache.header("Content-Encoding").is_some());

    let mut no_etag = Response::bytes(200, "text/plain", "cacheable body");
    maybe_gzip(&mut no_etag, context, &request, true).await;
    assert!(no_etag.header("Content-Encoding").is_some());

    let request = Request {
        headers: vec![Header {
            name: "Accept-Encoding".into(),
            value: "gzip".into(),
        }],
        ..request
    };
    let mut cacheable = Response::bytes(200, "text/plain", "cacheable body");
    cacheable.set_header("ETag", "\"present\"");
    maybe_gzip(&mut cacheable, context, &request, true).await;
    assert!(cacheable.header("Content-Encoding").is_none());
    let _ = std::fs::remove_dir_all(prefix);
}
