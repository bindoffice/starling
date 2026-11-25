use super::*;

#[tokio::test]
async fn routes_virtual_hosts_and_handles_ranges() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                error_log logs/error.log notice;
                pid logs/nginx.pid;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18081 default_server;
                        server_name first.test;
                        root html;
                        location / { return 200 "first\n"; }
                    }
                    server {
                        listen 127.0.0.1:18081;
                        server_name second.test;
                        root html;
                        location / { return 200 "second\n"; }
                    }
                    server {
                        listen 127.0.0.1:18081;
                        server_name *.wild.test;
                        root html;
                        location / { return 200 "wildcard\n"; }
                    }
                    server {
                        listen 127.0.0.1:18081;
                        server_name ~^api\.(?<domain>[a-z]+)\.test$;
                        root html;
                        location / { return 200 "$1/$domain\n"; }
                    }
                }
            "#,
        &[("html/index.html", "0123456789")],
    );
    let second = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: second.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(second.ends_with("second\n"));

    let wildcard_root = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: wild.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(wildcard_root.ends_with("wildcard\n"));

    let wildcard_child = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: child.wild.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(wildcard_child.ends_with("wildcard\n"));

    let regex_capture = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: api.example.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(regex_capture.ends_with("example/example\n"));

    let first = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: unknown.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(first.ends_with("first\n"));

    let (range_prefix, range_config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18082;
                        root html;
                        location / { }
                    }
                }
            "#,
        &[("html/data.bin", "0123456789")],
    );
    let range = request_body(
            &range_config,
            "GET /data.bin HTTP/1.1\r\nHost: localhost\r\nRange: bytes=2-5\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(range.starts_with("HTTP/1.1 206 Partial Content\r\n"));
    assert!(range.contains("Content-Range: bytes 2-5/10\r\n"));
    assert!(range.ends_with("2345"));
    let _ = std::fs::remove_dir_all(prefix);
    let _ = std::fs::remove_dir_all(range_prefix);
}

#[tokio::test]
async fn evaluates_if_conditions_and_set_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18083;
                        root html;
                        location /mode {
                            set $mode base;
                            if ($http_x_mode = special) {
                                set $mode special;
                                return 200 "$mode\n";
                            }
                            return 200 "$mode\n";
                        }
                        location /file {
                            if (-f $request_filename) {
                                return 204;
                            }
                            return 404;
                        }
                        location /regex-cap/ {
                            if ($uri ~ ^/regex-cap/(.*)$) {
                                return 200 "$1\n";
                            }
                            return 400;
                        }
                    }
                }
            "#,
        &[("html/file", "exists")],
    );
    let special = request_body(
        &config,
        "GET /mode HTTP/1.1\r\nHost: localhost\r\nX-Mode: special\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(special.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(special.ends_with("special\n"));

    let regular = request_body(
        &config,
        "GET /mode HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(regular.ends_with("base\n"));

    let file = request_body(
        &config,
        "GET /file HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(file.starts_with("HTTP/1.1 204 No Content\r\n"));

    let captures = request_body(
        &config,
        "GET /regex-cap/value HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(captures.ends_with("value\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn enforces_limit_except_method_access() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18095;
                        location /restricted {
                            limit_except GET {
                                deny all;
                            }
                            return 200 "$request_method\n";
                        }
                        location /inherited {
                            deny 127.0.0.0/8;
                            limit_except GET { }
                            return 200 "$request_method\n";
                        }
                        location /replaced {
                            deny 127.0.0.0/8;
                            limit_except GET {
                                allow 127.0.0.0/8;
                            }
                            return 200 "$request_method\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let get = request_body(
        &config,
        "GET /restricted HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(get.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(get.ends_with("GET\n"));

    let head = request_body(
        &config,
        "HEAD /restricted HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 200 OK\r\n"));

    let post = request_body(
            &config,
            "POST /restricted HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
    assert!(post.starts_with("HTTP/1.1 403 Forbidden\r\n"));

    let delete = request_body(
        &config,
        "DELETE /restricted HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(delete.starts_with("HTTP/1.1 403 Forbidden\r\n"));

    let inherited = request_body(
        &config,
        "POST /inherited HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(inherited.starts_with("HTTP/1.1 403 Forbidden\r\n"));

    let replaced = request_body(
        &config,
        "POST /replaced HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(replaced.starts_with("HTTP/1.1 200 OK\r\n"), "{replaced}");
    assert!(replaced.ends_with("POST\n"), "{replaced}");
    let _ = std::fs::remove_dir_all(prefix);
}
