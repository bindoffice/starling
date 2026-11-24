use super::support::*;

#[tokio::test]
async fn renders_html_json_jsonp_and_xml_autoindexes() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18161;
                    root html;
                    autoindex on;
                    location /json/ { autoindex_format json; }
                    location /xml/ { autoindex_format xml; }
                    location /jsonp/ { autoindex_format jsonp; }
                    location /html/ {
                        autoindex_format html;
                        autoindex_exact_size off;
                    }
                }
            }
        "#,
        &[
            ("html/json/dir/keep.txt", "x"),
            ("html/json/a.txt", "ab"),
            ("html/json/.hidden", "hidden"),
            ("html/xml/dir/keep.txt", "x"),
            ("html/xml/a.txt", "ab"),
            ("html/xml/.hidden", "hidden"),
            ("html/jsonp/dir/keep.txt", "x"),
            ("html/jsonp/a.txt", "ab"),
            ("html/html/dir/keep.txt", "x"),
            ("html/html/a.txt", "ab"),
        ],
    );

    let response = request_body(
        &config,
        "GET /json/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.contains("Content-Type: application/json\r\n"));
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.starts_with("[\r\n{ \"name\":\"dir\", \"type\":\"directory\""));
    assert!(body.contains("\"name\":\"a.txt\", \"type\":\"file\""));
    assert!(body.contains("\"size\":2"));
    assert!(!body.contains(".hidden"));
    assert!(body.find("\"name\":\"dir\"").unwrap() < body.find("\"name\":\"a.txt\"").unwrap());

    let response = request_body(
        &config,
        "GET /xml/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.contains("Content-Type: text/xml; charset=utf-8\r\n"));
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.starts_with("<?xml version=\"1.0\"?>\r\n<list>\r\n"));
    assert!(body.contains("<directory mtime=\""));
    assert!(body.contains("\">dir</directory>"));
    assert!(body.contains(" size=\"2\">a.txt</file>"));
    assert!(!body.contains(".hidden"));
    assert!(body.find("<directory").unwrap() < body.find("<file").unwrap());

    let response = request_body(
        &config,
        "GET /jsonp/?callback=foo.bar_9 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.contains("Content-Type: application/javascript\r\n"));
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.starts_with("/* callback */\r\nfoo.bar_9(["));
    assert!(body.ends_with("]);"));

    let response = request_body(
        &config,
        "GET /jsonp/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.contains("Content-Type: application/json\r\n"));
    assert!(std::str::from_utf8(body).unwrap().starts_with('['));

    let response = request_body(
        &config,
        "GET /jsonp/?callback=bad-name HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 400 Bad Request\r\n"));

    let response = request_body(
        &config,
        "GET /html/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (headers, body) = split_http_response(response.as_bytes());
    assert!(headers.contains("Content-Type: text/html\r\n"));
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.contains("<a href=\"../\">../</a>"));
    assert!(body.find(">dir/</a>").unwrap() < body.find(">a.txt</a>").unwrap());
    assert!(!body.contains(".hidden"));

    let _ = std::fs::remove_dir_all(prefix);
}

#[cfg(unix)]
#[tokio::test]
async fn follows_symlinks_for_autoindex_types_and_metadata() {
    use std::os::unix::fs::symlink;

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18162;
                    root html;
                    autoindex on;
                    location /json/ { autoindex_format json; }
                    location /xml/ { autoindex_format xml; }
                }
            }
        "#,
        &[("html/file.txt", "0123456789"), ("html/dir/keep", "x")],
    );
    for format in ["json", "xml"] {
        let directory = prefix.join("html").join(format);
        std::fs::create_dir_all(&directory).unwrap();
        symlink(prefix.join("html/file.txt"), directory.join("file-link")).unwrap();
        symlink(prefix.join("html/dir"), directory.join("dir-link")).unwrap();
        symlink(prefix.join("html/missing"), directory.join("broken-link")).unwrap();
    }
    symlink(prefix.join("html/file.txt"), prefix.join("html/file-link")).unwrap();
    symlink(prefix.join("html/dir"), prefix.join("html/dir-link")).unwrap();

    let response = request_body(
        &config,
        "GET /json/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(response.as_bytes());
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.contains("{ \"name\":\"file-link\", \"type\":\"file\", \"mtime\":\""));
    assert!(body.contains(", \"size\":10 }"));
    assert!(body.contains("{ \"name\":\"dir-link\", \"type\":\"directory\", \"mtime\":\""));
    assert!(body.contains("{ \"name\":\"broken-link\", \"type\":\"other\", \"mtime\":\""));

    let response = request_body(
        &config,
        "GET /xml/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(response.as_bytes());
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.contains("<file "));
    assert!(body.contains(" size=\"10\">file-link</file>"));
    assert!(body.contains("<directory "));
    assert!(body.contains(">dir-link</directory>"));
    assert!(body.contains("<other "));
    assert!(body.contains(">broken-link</other>"));

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let (_, body) = split_http_response(response.as_bytes());
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.contains("href=\"dir-link/\""));
    assert!(!body.contains("href=\"dir-link\""));

    let _ = std::fs::remove_dir_all(prefix);
}
