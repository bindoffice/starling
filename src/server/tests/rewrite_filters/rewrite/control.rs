use super::*;

#[tokio::test]
async fn handles_error_page_status_and_internal_method() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18086 default_server;
                        server_name preserve.test;
                        root html;
                        error_page 404 /errors/404.html;
                        location = /errors/404.html { internal; }
                        location / { try_files $uri =404; }
                    }
                    server {
                        listen 127.0.0.1:18086;
                        server_name response.test;
                        root html;
                        error_page 404 = /fallback.html;
                        location = /fallback.html { internal; }
                        location / { try_files $uri =404; }
                    }
                    server {
                        listen 127.0.0.1:18086;
                        server_name overwrite.test;
                        root html;
                        error_page 404 =418 /fallback.html;
                        location = /fallback.html { internal; }
                        location / { try_files $uri =404; }
                    }
                }
            "#,
        &[
            ("html/errors/404.html", "custom not found\n"),
            ("html/fallback.html", "fallback\n"),
        ],
    );

    let preserved = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: preserve.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(preserved.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(preserved.ends_with("custom not found\n"));

    let head = request_body(
        &config,
        "HEAD /missing HTTP/1.1\r\nHost: preserve.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(head.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(head.ends_with("\r\n\r\n"), "{head}");

    let post = request_body(
            &config,
            "POST /missing HTTP/1.1\r\nHost: preserve.test\r\nContent-Length: 4\r\nConnection: close\r\n\r\ndata",
        )
        .await;
    assert!(post.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(post.ends_with("custom not found\n"));

    let response = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: response.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with("fallback\n"));

    let overwritten = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: overwrite.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(overwritten.starts_with("HTTP/1.1 418 Unknown Status\r\n"));
    assert!(overwritten.ends_with("fallback\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn matched_if_keeps_the_selected_content_handler() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18461;
                        location /proxy-inside-if {
                            set $true 1;
                            if ($true) {
                                proxy_pass http://127.0.0.1:19020;
                            }
                            if ($true) {
                            }
                        }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET /proxy-inside-if HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 500 Internal Server Error\r\n"),
        "{response}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn failed_regex_matches_clear_numeric_captures() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18462;
                        location /stale-rewrite {
                            rewrite ^(.*)$ $1;
                            rewrite ^/never$ /second;
                            return 200 "$1\n";
                        }
                        location /stale-if {
                            if ($arg_a ~ ^(yes)(ok)$) {
                                set $mark "$1-$2";
                            }
                            if ($arg_b ~ ^(no)(pe)$) {
                            }
                            return 200 "$1|$mark\n";
                        }
                        location /negated {
                            if ($arg_c !~ ^(neg)(ated)$) {
                                return 204;
                            }
                            return 200 "$1|$2\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let rewrite = request_body(
        &config,
        "GET /stale-rewrite/abc HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(rewrite.ends_with("\r\n\r\n\n"), "{rewrite}");

    let no_match = request_body(
        &config,
        "GET /stale-if?a=yesok HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(no_match.ends_with("|yes-ok\n"), "{no_match}");

    let second_match = request_body(
        &config,
        "GET /stale-if?a=yesok&b=nope HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(second_match.ends_with("no|yes-ok\n"), "{second_match}");

    let negated = request_body(
        &config,
        "GET /negated?c=negated HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(negated.ends_with("neg|ated\n"), "{negated}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn matched_if_refreshes_path_variables_before_actions() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18463;
                        location /root {
                            root html/parent;
                            if ($arg_new) {
                                root html/child;
                                set $inside $document_root;
                            }
                            return 200 "$inside|$document_root|$request_filename\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET /root?new=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let root = prefix.join("html/child");
    let expected = format!(
        "{}|{}|{}\n",
        root.display(),
        root.display(),
        root.join("root").display()
    );
    assert!(response.ends_with(&expected), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn executes_return_directives_in_named_locations() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18102;
                        error_page 404 @error_named;
                        location /error/ {
                            try_files $uri =404;
                        }
                        location @error_named {
                            return 200 "error:$uri\n";
                        }
                        location /try/ {
                            try_files $uri @try_named;
                        }
                        location @try_named {
                            return 200 "try:$uri\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let error_page = request_body(
        &config,
        "GET /error/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        error_page.starts_with("HTTP/1.1 404 Not Found\r\n"),
        "unexpected named error response:\n{error_page}"
    );
    assert!(error_page.ends_with("error:/error/missing\n"));

    let try_files = request_body(
        &config,
        "GET /try/missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(try_files.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(try_files.ends_with("try:/try/missing\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn matches_nginx_return_and_scheme_redirect_semantics() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18134;
                        server_name configured.test;
                        location = /empty-location {
                            return 302;
                        }
                        location = /non-redirect {
                            return 300 "body";
                        }
                        location = /scheme-return {
                            return $scheme://example.test/from-scheme;
                        }
                        location = /scheme-rewrite {
                            rewrite ^ $scheme://example.test/from-rewrite;
                        }
                        location = /external-break {
                            rewrite ^ https://example.test/from-break break;
                        }
                        location = /drop-query {
                            rewrite ^ /target?x=1? last;
                        }
                        location = /target {
                            return 200 "$args\n";
                        }
                    }
                }
            "#,
        &[],
    );

    let empty_location = request_body(
        &config,
        "GET /empty-location HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(empty_location.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(empty_location.contains("Location: \r\n"));

    let non_redirect = request_body(
        &config,
        "GET /non-redirect HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(non_redirect.starts_with("HTTP/1.1 300 Unknown Status\r\n"));
    assert!(!non_redirect.contains("\r\nLocation:"));
    assert!(non_redirect.ends_with("body"));

    let scheme_return = request_body(
        &config,
        "GET /scheme-return HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(scheme_return.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(scheme_return.contains("Location: http://example.test/from-scheme\r\n"));

    let scheme_rewrite = request_body(
        &config,
        "GET /scheme-rewrite?keep=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(scheme_rewrite.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(scheme_rewrite.contains("Location: http://example.test/from-rewrite?keep=1\r\n"));

    let external_break = request_body(
        &config,
        "GET /external-break HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(external_break.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(external_break.contains("Location: https://example.test/from-break\r\n"));

    let drop_query = request_body(
        &config,
        "GET /drop-query?old=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(drop_query.ends_with("x=1\n"));
    let _ = std::fs::remove_dir_all(prefix);
}
