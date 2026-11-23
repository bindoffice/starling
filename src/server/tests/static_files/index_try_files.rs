use super::*;

#[tokio::test]
async fn updates_index_uri_redirects_loops_and_checks_try_files_types() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18152;
                        server_name localhost;
                        add_header X-URI $uri always;
                        root html;
                        location / { }
                        location /redirect/ {
                            index /re.html;
                        }
                        location /loop/ {
                            index /loop/;
                        }
                        location /many/ {
                            index missing.html many.html;
                        }
                        location /var/ {
                            index $server_name.html;
                        }
                        location /dir-dir/ {
                            try_files /directory/ =404;
                        }
                        location /dir-file/ {
                            try_files /directory =404;
                        }
                    }
                }
            "#,
        &[
            ("html/index.html", "body"),
            ("html/re.html", "rebody"),
            ("html/many/many.html", "manybody"),
            ("html/var/localhost.html", "varbody"),
        ],
    );
    std::fs::create_dir_all(prefix.join("html/directory")).unwrap();

    let default = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(default.contains("X-URI: /index.html\r\n"), "{default}");
    assert!(default.ends_with("body"), "{default}");

    let redirected = request_body(
        &config,
        "GET /redirect/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(redirected.contains("X-URI: /re.html\r\n"), "{redirected}");
    assert!(redirected.ends_with("rebody"), "{redirected}");

    let looped = request_body(
        &config,
        "GET /loop/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        looped.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{looped}"
    );

    let many = request_body(
        &config,
        "GET /many/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(many.contains("X-URI: /many/many.html\r\n"), "{many}");
    assert!(many.ends_with("manybody"), "{many}");

    let variable = request_body(
        &config,
        "GET /var/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        variable.contains("X-URI: /var/localhost.html\r\n"),
        "{variable}"
    );
    assert!(variable.ends_with("varbody"), "{variable}");

    let directory = request_body(
        &config,
        "GET /dir-dir/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        directory.starts_with("HTTP/1.1 301 Moved Permanently\r\n"),
        "{directory}"
    );

    let file = request_body(
        &config,
        "GET /dir-file/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(file.starts_with("HTTP/1.1 404 Not Found\r\n"), "{file}");
    let _ = std::fs::remove_dir_all(prefix);
}
