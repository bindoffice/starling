use super::support::*;
use super::*;

async fn serve_plain_response(listener: tokio::net::TcpListener, requests: usize) {
    for _ in 0..requests {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\
                  ETag: \"v1\"\r\nLast-Modified: Wed, 21 Oct 2015 07:28:00 GMT\r\n\
                  Accept-Ranges: bytes\r\n\r\n0123456789",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn forces_single_byte_ranges_for_buffered_proxy_responses() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(serve_plain_response(listener, 3));
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18147;
                    location = /forced {{
                        proxy_pass http://{backend_address};
                        proxy_force_ranges on;
                    }}
                    location = /plain {{
                        proxy_pass http://{backend_address};
                    }}
                    location = /advertised {{
                        proxy_pass http://{backend_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let request = |path: &str| {
        format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nRange: bytes=2-5\r\n\
             Connection: close\r\n\r\n"
        )
    };

    let forced = request_body(&config, &request("/forced")).await;
    assert!(
        forced.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{forced}"
    );
    assert!(
        forced.contains("Content-Range: bytes 2-5/10\r\n"),
        "{forced}"
    );
    assert!(forced.ends_with("2345"), "{forced}");

    let plain = request_body(&config, &request("/plain")).await;
    assert!(plain.starts_with("HTTP/1.1 200 OK\r\n"), "{plain}");
    assert!(plain.ends_with("0123456789"), "{plain}");

    let advertised = request_body(&config, &request("/advertised")).await;
    assert!(
        advertised.starts_with("HTTP/1.1 200 OK\r\n"),
        "{advertised}"
    );
    assert!(advertised.ends_with("0123456789"), "{advertised}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn add_header_accept_ranges_enables_range_filtering() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut byte = [0_u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\
                  Connection: close\r\n\r\n0123456789",
            )
            .await
            .unwrap();
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18638;
                    location / {{
                        proxy_pass http://{backend_address};
                        add_header Accept-Ranges bytes;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);

    let ranged = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nRange: bytes=2-5\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert!(
        ranged.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{ranged}"
    );
    assert!(ranged.contains("Accept-Ranges: bytes\r\n"), "{ranged}");
    assert!(
        ranged.contains("Content-Range: bytes 2-5/10\r\n"),
        "{ranged}"
    );
    assert!(ranged.ends_with("2345"), "{ranged}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_if_range_errors_and_head_to_forced_proxy_ranges() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(serve_plain_response(listener, 7));
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18472;
                    location / {{
                        proxy_pass http://{backend_address};
                        proxy_force_ranges on;
                        max_ranges 2;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let request = |method: &str, headers: &str| {
        format!(
            "{method} / HTTP/1.1\r\nHost: localhost\r\n{headers}\
             Connection: close\r\n\r\n"
        )
    };

    let matching_etag = request_body(
        &config,
        &request("GET", "Range: bytes=2-5\r\nIf-Range: \"v1\"\r\n"),
    )
    .await;
    assert!(matching_etag.starts_with("HTTP/1.1 206 Partial Content\r\n"));
    assert!(matching_etag.ends_with("2345"), "{matching_etag}");

    let stale_etag = request_body(
        &config,
        &request("GET", "Range: bytes=2-5\r\nIf-Range: \"v2\"\r\n"),
    )
    .await;
    assert!(stale_etag.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(stale_etag.ends_with("0123456789"), "{stale_etag}");

    let matching_date = request_body(
        &config,
        &request(
            "GET",
            "Range: bytes=2-5\r\nIf-Range: Wed, 21 Oct 2015 07:28:00 GMT\r\n",
        ),
    )
    .await;
    assert!(
        matching_date.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{matching_date}"
    );
    assert!(matching_date.ends_with("2345"), "{matching_date}");

    let malformed = request_body(&config, &request("GET", "Range: bytes=broken\r\n")).await;
    assert!(
        malformed.starts_with("HTTP/1.1 416 Requested Range Not Satisfiable\r\n"),
        "{malformed}"
    );
    assert!(
        malformed.contains("Content-Range: bytes */10\r\n"),
        "{malformed}"
    );

    let multiple = request_body(&config, &request("GET", "Range: bytes=0-1,3-4\r\n")).await;
    assert!(multiple.starts_with("HTTP/1.1 200 OK\r\n"), "{multiple}");
    assert!(multiple.ends_with("0123456789"), "{multiple}");

    let uppercase_unit = request_body(&config, &request("GET", "Range: BYTES=3-6\r\n")).await;
    assert!(
        uppercase_unit.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{uppercase_unit}"
    );
    assert!(uppercase_unit.ends_with("3456"), "{uppercase_unit}");

    let head = request_body(&config, &request("HEAD", "Range: bytes=2-5\r\n")).await;
    assert!(
        head.starts_with("HTTP/1.1 206 Partial Content\r\n"),
        "{head}"
    );
    assert!(head.contains("Content-Length: 4\r\n"), "{head}");
    assert!(head.ends_with("\r\n\r\n"), "{head}");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn forces_ranges_for_fastcgi_uwsgi_and_scgi_responses() {
    let fastcgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let uwsgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let scgi_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let fastcgi_address = fastcgi_listener.local_addr().unwrap();
    let uwsgi_address = uwsgi_listener.local_addr().unwrap();
    let scgi_address = scgi_listener.local_addr().unwrap();

    let fastcgi = tokio::spawn(async move {
        let (mut stream, _) = fastcgi_listener.accept().await.unwrap();
        loop {
            let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        }
        write_fastcgi_record(
            &mut stream,
            FCGI_STDOUT,
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\n0123456789",
        )
        .await
        .unwrap();
        write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let uwsgi = tokio::spawn(async move {
        let (mut stream, _) = uwsgi_listener.accept().await.unwrap();
        let mut header = [0_u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let mut parameters = vec![0_u8; u16::from_le_bytes([header[1], header[2]]) as usize];
        stream.read_exact(&mut parameters).await.unwrap();
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\n0123456789",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });
    let scgi = tokio::spawn(async move {
        let (mut stream, _) = scgi_listener.accept().await.unwrap();
        let mut length = Vec::new();
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await.unwrap();
            if byte[0] == b':' {
                break;
            }
            length.push(byte[0]);
        }
        let length = std::str::from_utf8(&length)
            .unwrap()
            .parse::<usize>()
            .unwrap();
        let mut parameters = vec![0_u8; length];
        stream.read_exact(&mut parameters).await.unwrap();
        stream
            .write_all(
                b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 10\r\n\r\n0123456789",
            )
            .await
            .unwrap();
        stream.shutdown().await.unwrap();
    });

    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen 127.0.0.1:18473;
                    location = /fastcgi {{
                        fastcgi_pass {fastcgi_address};
                        fastcgi_force_ranges on;
                    }}
                    location = /uwsgi {{
                        uwsgi_pass {uwsgi_address};
                        uwsgi_force_ranges on;
                    }}
                    location = /scgi {{
                        scgi_pass {scgi_address};
                        scgi_force_ranges on;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    for path in ["/fastcgi", "/uwsgi", "/scgi"] {
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: localhost\r\nRange: bytes=3-6\r\n\
             Connection: close\r\n\r\n"
        );
        let response = request_body(&config, &request).await;
        assert!(
            response.starts_with("HTTP/1.1 206 Partial Content\r\n"),
            "{path}: {response}"
        );
        assert!(
            response.contains("Content-Range: bytes 3-6/10\r\n"),
            "{path}: {response}"
        );
        assert!(response.ends_with("3456"), "{path}: {response}");
    }
    fastcgi.await.unwrap();
    uwsgi.await.unwrap();
    scgi.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
