use super::support::*;

#[tokio::test]
async fn adds_before_and_after_bodies_with_types_and_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18173;
                        root html;
                        add_before_body /before;
                        add_after_body /after;
                        location = /before { internal; return 200 "[before]"; }
                        location = /after { internal; return 200 "[after]"; }
                        location = /page.html { }
                        location /plain/ {
                            addition_types text/plain;
                        }
                        location /any/ {
                            addition_types *;
                        }
                        location /dynamic/ {
                            add_after_body "/echo?source=$arg_source";
                        }
                        location = /echo { internal; return 200 "[$arg_source]"; }
                        location = /missing { return 404 "missing"; }
                    }
                }
            "#,
        &[
            ("html/page.html", "page\n"),
            ("html/plain/site.txt", "plain\n"),
            ("html/other.txt", "other\n"),
            ("html/dynamic/file.html", "dynamic\n"),
            ("html/any/blob.bin", "binary\n"),
        ],
    );

    let response = request_bytes(
        &config,
        "GET /page.html HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&response);
    assert_eq!(body, b"[before]page\n[after]");
    assert!(headers.contains("Content-Length: 20\r\n"), "{headers}");
    assert!(headers.contains("ETag: W/\""), "{headers}");
    assert!(!headers.contains("Accept-Ranges:"), "{headers}");

    let plain = request_bytes(
        &config,
        "GET /plain/site.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&plain);
    assert_eq!(body, b"[before]plain\n[after]");

    let other = request_bytes(
        &config,
        "GET /other.txt HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&other);
    assert_eq!(body, b"other\n");

    let any = request_bytes(
        &config,
        "GET /any/blob.bin HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&any);
    assert_eq!(body, b"[before]binary\n[after]");

    let dynamic = request_bytes(
        &config,
        "GET /dynamic/file.html?source=value HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(&dynamic);
    assert_eq!(body, b"[before]dynamic\n[value]");

    let missing = request_bytes(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(&missing);
    assert!(
        headers.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "{headers}"
    );
    assert_eq!(body, b"missing");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn compiles_addition_inheritance_and_validation() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    add_before_body /http-before;
                    addition_types text/html application/xhtml+xml;
                    server {
                        listen 127.0.0.1:18174;
                        add_after_body /server-after;
                        location /inherit/ { }
                        location /override/ {
                            add_before_body /local-before;
                            addition_types text/plain;
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(
        server.context.addition_before.as_deref(),
        Some("/http-before")
    );
    assert_eq!(
        server.context.addition_after.as_deref(),
        Some("/server-after")
    );
    assert_eq!(
        server.context.addition_types,
        ["text/html", "application/xhtml+xml"]
    );
    assert_eq!(
        server.locations[0].context.addition_before.as_deref(),
        Some("/http-before")
    );
    assert_eq!(
        server.locations[1].context.addition_before.as_deref(),
        Some("/local-before")
    );
    assert_eq!(server.locations[1].context.addition_types, ["text/plain"]);

    let duplicate = test_config_error(
        r#"
                events {}
                http {
                    server {
                        listen 127.0.0.1:18174;
                        add_before_body /first;
                        add_before_body /second;
                    }
                }
            "#,
    );
    assert!(
        duplicate.contains("\"add_before_body\" directive is duplicate"),
        "{duplicate}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
