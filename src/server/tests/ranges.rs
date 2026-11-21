use super::support::*;

#[tokio::test]
async fn serves_multipart_ranges_and_honors_max_ranges() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18561;
                    root html;
                    location / { }
                    location /limited {
                        max_ranges 2;
                    }
                    location /single {
                        max_ranges 1;
                    }
                    location /disabled {
                        max_ranges 0;
                    }
                }
            }
        "#,
        &[
            ("html/data.bin", "0123456789"),
            ("html/limited", "0123456789"),
            ("html/single", "0123456789"),
            ("html/disabled", "0123456789"),
        ],
    );
    let request = |method: &str, path: &str, range: &str, extra: &str| {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nRange: {range}\r\n{extra}\
             Connection: close\r\n\r\n"
        )
    };

    let default_multiple =
        request_body(&config, &request("GET", "/data.bin", "bytes=0-1,3-4", "")).await;
    assert!(
        default_multiple.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{default_multiple}"
    );
    assert!(
        default_multiple.contains("Content-Type: multipart/byteranges; boundary="),
        "{default_multiple}"
    );

    let multiple = request_body(&config, &request("GET", "/limited", "bytes=0-1,3-4", "")).await;
    assert!(
        multiple.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{multiple}"
    );
    let boundary = multipart_boundary(&multiple);
    let expected = format!(
        "\r\n--{boundary}\r\nContent-Type: text/plain\r\n\
         Content-Range: bytes 0-1/10\r\n\r\n01\
         \r\n--{boundary}\r\nContent-Type: text/plain\r\n\
         Content-Range: bytes 3-4/10\r\n\r\n34\
         \r\n--{boundary}--\r\n"
    );
    assert!(multiple.ends_with(&expected), "{multiple}");
    assert!(
        multiple.contains(&format!("Content-Length: {}\r\n", expected.len())),
        "{multiple}"
    );

    let exceeded = request_body(&config, &request("GET", "/single", "bytes=0-1,3-4", "")).await;
    assert!(exceeded.starts_with("HTTP/1.1 200 OK\r\n"), "{exceeded}");
    assert!(exceeded.ends_with("0123456789"));

    let disabled = request_body(
        &config,
        &request("GET", "/disabled", "bytes=0-1,broken", ""),
    )
    .await;
    assert!(disabled.starts_with("HTTP/1.1 200 OK\r\n"), "{disabled}");
    assert!(disabled.ends_with("0123456789"));

    let partial = request_body(&config, &request("GET", "/limited", "bytes=0-1,20-30", "")).await;
    assert!(
        partial.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{partial}"
    );
    assert!(partial.contains("Content-Range: bytes 0-1/10\r\n"));
    assert!(partial.ends_with("01"));

    let stale = request_body(
        &config,
        &request(
            "GET",
            "/limited",
            "bytes=0-1,3-4",
            "If-Range: \"stale\"\r\n",
        ),
    )
    .await;
    assert!(stale.starts_with("HTTP/1.1 200 OK\r\n"), "{stale}");
    assert!(stale.ends_with("0123456789"));

    let unsatisfiable = request_body(
        &config,
        &request("GET", "/limited", "bytes=20-30,40-50", ""),
    )
    .await;
    assert!(
        unsatisfiable.starts_with("HTTP/1.1 416 Requested Range Not Satisfiable\r\n"),
        "{unsatisfiable}"
    );
    assert!(unsatisfiable.contains("Content-Range: bytes */10\r\n"));

    let head = request_bytes(&config, &request("HEAD", "/limited", "bytes=0-1,3-4", "")).await;
    let (headers, body) = split_http_response(&head);
    assert!(headers.starts_with("HTTP/1.1 206 Partial Content"));
    assert!(headers.contains("Content-Type: multipart/byteranges; boundary="));
    assert!(body.is_empty());

    let _ = std::fs::remove_dir_all(prefix);
}

fn multipart_boundary(response: &str) -> String {
    response
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Type: multipart/byteranges; boundary=")
                .map(str::to_string)
        })
        .expect("multipart boundary")
}
