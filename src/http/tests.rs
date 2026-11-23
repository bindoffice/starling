use super::*;
use crate::config::{ServerTokens, TempPathConfig};
use bytes::Bytes;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::BufReader;

static NEXT_BUFFER_TEMP: AtomicU64 = AtomicU64::new(1);

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

#[tokio::test]

async fn parses_request_and_content_length() {
    let input =
        b"GET /hello?name=x HTTP/1.1\r\nHost: example.test\r\nConnection: keep-alive\r\n\r\n";

    let mut reader = BufReader::new(&input[..]);

    let request = read_request(
        &mut reader,
        "127.0.0.1:1234".into(),
        "127.0.0.1:80".into(),
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(request.method, "GET");

    assert_eq!(request.host_without_port(), "example.test");

    assert!(request.keep_alive);
}

#[tokio::test]
async fn parses_connect_authority_with_a_required_port() {
    let input = b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n";
    let mut reader = BufReader::new(&input[..]);

    let head = read_request_head(&mut reader).await.unwrap().unwrap();

    assert_eq!(head.method, "CONNECT");
    assert_eq!(head.target, "example.test:443");

    let missing_port = b"CONNECT example.test HTTP/1.1\r\nHost: example.test\r\n\r\n";
    let mut reader = BufReader::new(&missing_port[..]);
    let error = read_request_head(&mut reader).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn exposes_request_line_ports_without_using_the_host_header() {
    let absolute = b"GET http://example.test:8080/hello HTTP/1.1\r\nHost: proxy.test\r\n\r\n";
    let mut reader = BufReader::new(&absolute[..]);
    let request = read_request(
        &mut reader,
        "127.0.0.1:1234".into(),
        "127.0.0.1:80".into(),
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(request.host_without_port(), "example.test");
    assert_eq!(request.request_port(), Some(8080));

    let origin = b"GET /hello HTTP/1.1\r\nHost: example.test:8080\r\n\r\n";
    let mut reader = BufReader::new(&origin[..]);
    let request = read_request(
        &mut reader,
        "127.0.0.1:1234".into(),
        "127.0.0.1:80".into(),
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(request.request_port(), None);
}

#[tokio::test]
async fn normalizes_and_validates_request_hosts_like_nginx() {
    let cases = [
        ("GET / HTTP/1.0\r\nHost: L.\r\n\r\n", Some("l"), None),
        (
            "GET / HTTP/1.0\r\nHost: abcd-ef.g02.xyz:\r\n\r\n",
            Some("abcd-ef.g02.xyz"),
            None,
        ),
        (
            "GET http://AbC.Example.:8080/x?args HTTP/1.0\r\nHost: ignored\r\n\r\n",
            Some("abc.example"),
            Some(8080),
        ),
        (
            "GET http://[::FFFF:12.30.67.89]:4321/ HTTP/1.0\r\nHost: ignored\r\n\r\n",
            Some("[::ffff:12.30.67.89]"),
            Some(4321),
        ),
    ];
    for (input, host, port) in cases {
        let mut reader = BufReader::new(input.as_bytes());
        let request = read_request(
            &mut reader,
            "127.0.0.1:1234".into(),
            "127.0.0.1:80".into(),
            Duration::from_secs(1),
            1024,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            request.host_without_port().as_ref(),
            host.unwrap(),
            "{input}"
        );
        assert_eq!(request.request_port(), port, "{input}");
    }

    let invalid_targets = [
        "GET http:/// HTTP/1.0\r\nHost: localhost\r\n\r\n",
        "GET http://example.com.:1.2/ HTTP/1.0\r\nHost: localhost\r\n\r\n",
        "GET http://123.40.56.78:9000:80/ HTTP/1.0\r\nHost: localhost\r\n\r\n",
        "GET http://extra[abcd::ef98:0:7654:321]/ HTTP/1.0\r\nHost: localhost\r\n\r\n",
    ];
    for input in invalid_targets {
        let mut reader = BufReader::new(input.as_bytes());
        let error = read_request_head(&mut reader).await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{input:?}");
    }

    let invalid_hosts = [
        "GET / HTTP/1.0\r\nHost: [abcd::ef98:0:7654:321].\r\n\r\n",
        "GET / HTTP/1.0\r\nHost: \u{2}\r\n\r\n",
    ];
    for input in invalid_hosts {
        let mut reader = BufReader::new(input.as_bytes());
        let request = read_request_head(&mut reader)
            .await
            .unwrap()
            .expect("request head");
        assert!(request.invalid_host, "{input:?}");
    }
}

#[test]
fn extracts_paths_and_args_from_absolute_targets() {
    let cases = [
        ("http://localhost", "/", None),
        ("http://localhost?args", "/", Some("args")),
        ("http://localhost?args#frag", "/", Some("args")),
        ("http://localhost:8080/a/b?x=1#frag", "/a/b", Some("x=1")),
        ("/foo/bar/.#frag", "/foo/bar/.", None),
        ("/foo/bar/..#frag?ignored", "/foo/bar/..", None),
    ];
    for (target, path, args) in cases {
        let request = Request {
            method: "GET".into(),
            target: target.into(),
            version: "HTTP/1.1".into(),
            headers: Vec::new(),
            body: Vec::new(),
            body_stream: None,
            remote_addr: "127.0.0.1:1".into(),
            local_addr: "127.0.0.1:2".into(),
            keep_alive: false,
        };
        assert_eq!(request.path_and_query(), (path, args), "{target}");
    }
}

#[tokio::test]

async fn parses_chunked_request_body() {
    let input =
        b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n0\r\n\r\n";

    let mut reader = BufReader::new(&input[..]);

    let request = read_request(
        &mut reader,
        "127.0.0.1:1234".into(),
        "127.0.0.1:80".into(),
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(request.body, b"test");
}

#[tokio::test]
async fn parses_chunked_upstream_trailers() {
    let input =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n0\r\nX-Checksum: abc\r\nX-Multi: one\r\nX-Multi: two\r\n\r\n";
    let mut reader = BufReader::new(&input[..]);

    let response = read_upstream_response(&mut reader).await.unwrap();

    assert_eq!(response.body.into_memory(16).await.unwrap(), b"test");
    assert_eq!(response.trailers.len(), 3);
    assert_eq!(response.trailers[0].name, "X-Checksum");
    assert_eq!(response.trailers[0].value, "abc");
    assert_eq!(response.trailers[2].value, "two");
}

#[tokio::test]
async fn rejects_invalid_chunked_upstream_trailers() {
    let input =
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nInvalid-Trailer\r\n\r\n";
    let mut reader = BufReader::new(&input[..]);

    let error = read_upstream_response(&mut reader).await.unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn rejects_unterminated_chunked_request_trailers() {
    let input = b"POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n\
                  0\r\nX-Trace: one\r\n";
    let mut reader = BufReader::new(&input[..]);

    let error = read_request(
        &mut reader,
        "127.0.0.1:1234".into(),
        "127.0.0.1:80".into(),
        Duration::from_secs(1),
        1024,
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn rejects_unterminated_chunked_upstream_trailers() {
    let input = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                  0\r\nX-Trace: one\r\n";
    let mut reader = BufReader::new(&input[..]);

    let error = read_upstream_response(&mut reader).await.unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
}

#[tokio::test]
async fn spills_upstream_bodies_to_a_temp_file_and_cleans_them_up() {
    let id = NEXT_BUFFER_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp_dir =
        std::env::temp_dir().join(format!("starling-http-buffer-{}-{id}", std::process::id()));
    let input = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\nabcdefgh";
    let mut reader = BufReader::new(&input[..]);
    let options = UpstreamBodyBufferOptions {
        memory_limit: 4,
        busy_limit: usize::MAX,
        buffer_count: 1,
        buffer_size: 4,
        temp_path: Some(TempPathConfig::new(temp_dir.clone())),
        max_temp_file_size: 32,
        write_size: 2,
    };

    let (mut response, _) =
        read_upstream_response_timed_with_options(&mut reader, 0, MAX_HEADER_BYTES, options)
            .await
            .unwrap();
    assert!(matches!(
        &response.body,
        ResponseBody::FileWithPrefix { .. }
    ));
    let temp_file = response
        .body_temp_file
        .as_ref()
        .expect("temp file guard")
        .path()
        .to_path_buf();
    assert!(temp_file.exists());

    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    assert_eq!(body.into_memory(64).await.unwrap(), b"abcdefgh");
    drop(response);
    assert!(!temp_file.exists());
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn rejects_upstream_bodies_over_the_temp_file_limit() {
    let id = NEXT_BUFFER_TEMP.fetch_add(1, Ordering::Relaxed);
    let temp_dir = std::env::temp_dir().join(format!(
        "starling-http-buffer-limit-{}-{id}",
        std::process::id()
    ));
    let input = b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nabcdefghi";
    let mut reader = BufReader::new(&input[..]);
    let options = UpstreamBodyBufferOptions {
        memory_limit: 4,
        busy_limit: usize::MAX,
        buffer_count: 1,
        buffer_size: 4,
        temp_path: Some(TempPathConfig::new(temp_dir.clone())),
        max_temp_file_size: 4,
        write_size: 2,
    };

    let error =
        read_upstream_response_timed_with_options(&mut reader, 0, MAX_HEADER_BYTES, options)
            .await
            .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("temp file size"));
    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn streaming_failure_after_headers_does_not_rewrite_the_response() {
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    sender
        .send(Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "simulated streaming body truncation",
        )))
        .await
        .unwrap();
    let mut response = Response::new(200);
    response.set_header("Content-Type", "text/plain");
    response.set_header("Content-Length", "10");
    response.body = ResponseBody::Stream(ResponseStream::new(receiver, Some(10), false));

    let mut output = Vec::new();
    let error = write_response(
        &mut output,
        &mut response,
        ResponseWriteOptions {
            request_method: "GET",
            request_version: "HTTP/1.1",
            keep_alive: true,
            keep_alive_header: None,
            server_tokens: ServerTokens::On,
            chunked_transfer_encoding: true,
            rate: ResponseRate::default(),
        },
    )
    .await
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    let output = String::from_utf8(output).unwrap();
    assert!(output.starts_with("HTTP/1.1 200 OK\r\n"), "{output}");
    assert!(output.contains("\r\n\r\nfirst"), "{output}");
    assert!(!output.contains("502 Bad Gateway"), "{output}");
    assert!(!output.contains("504 Gateway Time-out"), "{output}");
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
