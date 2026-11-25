use super::*;

use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::BufReader;

mod body;
mod request;

static NEXT_BUFFER_TEMP: AtomicU64 = AtomicU64::new(1);

#[test]
fn preserves_opaque_header_value_bytes_without_lossy_utf8_conversion() {
    let bytes: Vec<u8> = (0..=255)
        .filter(|byte| !matches!(*byte, 0 | b'\r' | b'\n'))
        .collect();
    let value = header_value_from_bytes(&bytes);

    assert_eq!(header_value_bytes(&value).as_ref(), bytes.as_slice());
}

#[test]
fn leaves_utf8_header_values_unencoded() {
    let value = header_value_from_bytes("plain ascii".as_bytes());

    assert_eq!(value, "plain ascii");
    assert_eq!(header_value_bytes(&value).as_ref(), b"plain ascii");
}

#[test]
fn decodes_only_values_with_the_internal_raw_prefix() {
    let embedded_nul = "prefix\0starling-raw:zz\0";
    assert_eq!(
        header_value_bytes(embedded_nul).as_ref(),
        embedded_nul.as_bytes()
    );

    let encoded = header_value_from_bytes(&[0xff, 0x00, 0xfe]);
    assert_eq!(
        header_value_bytes(&encoded).as_ref(),
        [0xff, 0x00, 0xfe].as_slice()
    );
}

#[test]
fn canonicalizes_known_header_names_without_scanning_the_whole_set() {
    let names = [
        "Content-Type",
        "Content-Length",
        "Date",
        "Server",
        "Last-Modified",
        "ETag",
        "Connection",
        "Keep-Alive",
        "Transfer-Encoding",
        "Cache-Control",
        "Location",
        "Set-Cookie",
        "Vary",
        "Host",
        "User-Agent",
        "Accept",
        "Accept-Encoding",
        "Accept-Language",
        "Accept-Ranges",
        "Authorization",
        "Cookie",
        "Referer",
        "Range",
        "If-Modified-Since",
        "If-None-Match",
        "Allow",
        "Content-Disposition",
        "Content-Encoding",
        "Content-Language",
        "Content-Location",
        "Content-Range",
        "Expect",
        "Expires",
        "Link",
        "P3P",
        "Proxy-Connection",
        "TE",
        "Trailer",
        "Upgrade",
        "Via",
        "WWW-Authenticate",
        "X-Accel-Buffering",
        "X-Accel-Charset",
        "X-Accel-Limit-Rate",
        "X-Accel-Redirect",
        "X-Forwarded-For",
        "X-Forwarded-Proto",
        "X-Real-IP",
    ];

    for name in names {
        assert_eq!(HeaderName::from(name.to_ascii_lowercase()).as_str(), name);
        assert_eq!(HeaderName::from(name).as_str(), name);
    }
    assert_eq!(
        HeaderName::from("X-Custom-Header").as_str(),
        "X-Custom-Header"
    );
}

#[test]
fn loads_small_static_files_into_memory_and_streams_large_files() {
    let id = NEXT_BUFFER_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "starling-http-file-response-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();

    let small_path = temp_dir.join("small");
    std::fs::write(&small_path, b"Hello, World!\n").unwrap();
    let small = file_response(&small_path, "text/plain".into()).unwrap();
    assert!(matches!(
        small.body,
        ResponseBody::Bytes(ref body) if body == b"Hello, World!\n"
    ));
    assert_eq!(small.header("content-length"), Some("14"));

    let large_path = temp_dir.join("large");
    std::fs::write(&large_path, vec![b'x'; 64 * 1024 + 1]).unwrap();
    let large = file_response(&large_path, "application/octet-stream".into()).unwrap();
    assert!(matches!(large.body, ResponseBody::File { .. }));
    assert_eq!(large.header("content-length"), Some("65537"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn small_static_file_reads_stop_at_the_reported_length() {
    let id = NEXT_BUFFER_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "starling-http-file-length-{}-{id}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let path = temp_dir.join("small");
    std::fs::write(&path, b"abc").unwrap();

    let file = std::fs::File::open(&path).unwrap();
    let metadata = file.metadata().unwrap();
    std::fs::write(&path, b"a").unwrap();
    let response = file_response_with_metadata(file, &metadata, "text/plain".into()).unwrap();

    assert!(matches!(
        response.body,
        ResponseBody::Bytes(ref body) if body == b"a"
    ));
    assert_eq!(response.header("content-length"), Some("1"));
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]

async fn rejects_malformed_request_headers() {
    let cases = [
        "GET / HTTP/1.1\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: one\r\nHost: two\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: one\r\nhOsT: two\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nContent-Length: 1\r\nTransfer-Encoding: chunked\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: chunked\r\ntRaNsFeR-EnCoDiNg: chunked\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: foo\r\nTransfer-Encoding: bar\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\nContent-Length: 1\r\nTransfer-Encoding: identity\r\n\r\n",
        "GET / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n",
        "GET / HTTP/2.0\r\nHost: example.test\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: example.test\r\n X-Folded: yes\r\n\r\n",
    ];

    for input in cases {
        let mut reader = BufReader::new(input.as_bytes());

        let error = read_request_head(&mut reader).await.unwrap_err();

        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidData,
            "input should be rejected: {input:?}"
        );
    }

    for input in [
        "GET / HTTP/1.1\r\nHost: bad host\r\n\r\n",
        "GET / HTTP/1.1\r\nHost: bad..host\r\n\r\n",
    ] {
        let mut reader = BufReader::new(input.as_bytes());
        let request = read_request_head(&mut reader)
            .await
            .unwrap()
            .expect("request head");
        assert!(
            request.invalid_host,
            "input should carry an invalid Host: {input:?}"
        );
    }
}

#[tokio::test]
async fn rejects_duplicate_unique_request_headers_case_insensitively() {
    let cases = [
        ("host", "example.test"),
        ("if-modified-since", "Sun, 06 Nov 1994 08:49:37 GMT"),
        ("if-unmodified-since", "Sun, 06 Nov 1994 08:49:37 GMT"),
        ("if-match", "\"etag\""),
        ("if-none-match", "\"etag\""),
        ("content-length", "0"),
        ("content-range", "bytes 0-0/1"),
        ("if-range", "\"etag\""),
        ("transfer-encoding", "chunked"),
        ("expect", "100-continue"),
        ("authorization", "Basic dGVzdDp0ZXN0"),
    ];

    for (name, value) in cases {
        let duplicate = name.to_ascii_uppercase();
        let input = format!(
            "GET / HTTP/1.1\r\nHost: example.test\r\n{name}: {value}\r\n{duplicate}: {value}\r\n\r\n"
        );
        let mut reader = BufReader::new(input.as_bytes());

        let error = read_request_head(&mut reader).await.unwrap_err();

        assert_eq!(
            error.kind(),
            io::ErrorKind::InvalidData,
            "duplicate {name} should be rejected case-insensitively"
        );
    }
}

#[tokio::test]
async fn classifies_unknown_transfer_encodings_as_not_implemented() {
    for value in ["identity", "chunked, identity"] {
        let input =
            format!("GET / HTTP/1.1\r\nHost: example.test\r\nTransfer-Encoding: {value}\r\n\r\n");
        let mut reader = BufReader::new(input.as_bytes());

        let error = read_request_head(&mut reader).await.unwrap_err();

        assert_eq!(
            error.kind(),
            io::ErrorKind::Unsupported,
            "unsupported Transfer-Encoding should map to 501: {value:?}"
        );
    }
}
