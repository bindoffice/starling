use super::support::*;
use super::*;

#[tokio::test]
async fn adds_charset_without_recode_when_source_charset_is_off() {
    let source = r#"
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18547;
                root html;
                charset utf-8;
                charset_types text/plain;
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[("html/note.txt", "")]);
    std::fs::write(prefix.join("html/note.txt"), [0xc1, b'\n']).unwrap();

    let response = request_bytes(
        &config,
        "GET /note.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{headers}"
    );
    assert_eq!(body, [0xc1, b'\n']);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn recodes_charset_map_in_both_directions() {
    let source = r#"
        events { worker_connections 64; }
        http {
            charset_map koi8-r utf-8 {
                C1 D0B0;
                9C C2B0;
            }
            server {
                listen 127.0.0.1:18544;
                root html;
                charset_types text/plain;
                location = /koi8.txt {
                    source_charset koi8-r;
                    charset utf-8;
                }
                location = /utf8.txt {
                    source_charset utf-8;
                    charset koi8-r;
                }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    std::fs::create_dir_all(prefix.join("html")).unwrap();
    std::fs::write(prefix.join("html/koi8.txt"), [0xc1, 0x9c, b'\n']).unwrap();
    std::fs::write(
        prefix.join("html/utf8.txt"),
        [0xd0, 0xb0, 0xc2, 0xb0, b'\n'],
    )
    .unwrap();

    let response = request_bytes(
        &config,
        "GET /koi8.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{headers}"
    );
    assert!(headers.contains("Content-Length: 5\r\n"), "{headers}");
    assert_eq!(body, [0xd0, 0xb0, 0xc2, 0xb0, b'\n']);

    let response = request_bytes(
        &config,
        "GET /utf8.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain; charset=koi8-r\r\n"),
        "{headers}"
    );
    assert_eq!(body, [0xc1, 0x9c, b'\n']);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn leaves_body_unchanged_without_a_runtime_mapping() {
    let source = r#"
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18545;
                root html;
                set $target utf-8;
                source_charset koi8-r;
                charset $target;
                charset_types text/plain;
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[("html/note.txt", "")]);
    std::fs::write(prefix.join("html/note.txt"), [0xc1, b'\n']).unwrap();

    let response = request_bytes(
        &config,
        "GET /note.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain\r\n"),
        "{headers}"
    );
    assert!(!headers.contains("charset="), "{headers}");
    assert_eq!(body, [0xc1, b'\n']);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn honors_override_charset_and_content_encoding() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for encoding in [None, None, Some("gzip")] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0_u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            let mut response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=koi8-r\r\nContent-Length: 1\r\n"
                .to_vec();
            if let Some(encoding) = encoding {
                response.extend_from_slice(format!("Content-Encoding: {encoding}\r\n").as_bytes());
            }
            response.extend_from_slice(b"\r\n\xc1");
            stream.write_all(&response).await.unwrap();
            stream.shutdown().await.unwrap();
        }
    });
    let source = format!(
        r#"
            events {{ worker_connections 64; }}
            http {{
                charset_map koi8-r utf-8 {{
                    C1 D0B0;
                }}
                server {{
                    listen 127.0.0.1:18546;
                    source_charset koi8-r;
                    charset utf-8;
                    charset_types text/plain;
                    location = /off {{
                        proxy_pass http://{backend_address};
                    }}
                    location = /on {{
                        override_charset on;
                        proxy_pass http://{backend_address};
                    }}
                    location = /gzip {{
                        override_charset on;
                        proxy_pass http://{backend_address};
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);

    let response = request_bytes(
        &config,
        "GET /off HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain; charset=koi8-r\r\n"),
        "{headers}"
    );
    assert_eq!(body, [0xc1]);

    let response = request_bytes(
        &config,
        "GET /on HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(
        headers.contains("Content-Type: text/plain; charset=utf-8\r\n"),
        "{headers}"
    );
    assert_eq!(body, [0xd0, 0xb0]);

    let response = request_bytes(
        &config,
        "GET /gzip HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert!(headers.contains("Content-Encoding: gzip\r\n"), "{headers}");
    assert!(
        headers.contains("Content-Type: text/plain; charset=koi8-r\r\n"),
        "{headers}"
    );
    assert_eq!(body, [0xc1]);

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
