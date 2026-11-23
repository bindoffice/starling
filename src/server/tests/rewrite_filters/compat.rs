use super::*;

#[tokio::test]
async fn matches_nginx_rewrite_args_returns_and_error_pages() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18151;
                        server_name localhost;
                        location /add {
                            rewrite ^ http://example.com/?c=d redirect;
                        }
                        location /drop {
                            rewrite ^ http://example.com/?c=d? redirect;
                        }
                        location /unescape {
                            rewrite ^ $arg_r? redirect;
                        }
                        location /capturedup {
                            rewrite ^(.*)$ $1?c=$1;
                            return 200 "uri:$uri args:$args";
                        }
                        location /return405 {
                            return 405;
                        }
                        location /error404return405 {
                            error_page 404 /return405;
                            return 404;
                        }
                        location /return200text {
                            return 200 "text";
                        }
                        location /return302text {
                            return 302 "text";
                        }
                        location /error405return200text {
                            error_page 405 /return200text;
                            return 405;
                        }
                        location /error302return200text {
                            error_page 302 /return200text;
                            return 302 "text";
                        }
                        location /error405return302 {
                            error_page 405 /return302text;
                            return 405;
                        }
                        location /auto/ {
                            proxy_pass http://127.0.0.1:9;
                        }
                    }
                }
            "#,
        &[],
    );

    let added = request_body(
        &config,
        "GET /add?a=b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        added.contains("Location: http://example.com/?c=d&a=b\r\n"),
        "{added}"
    );

    let dropped = request_body(
        &config,
        "GET /drop?a=b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        dropped.contains("Location: http://example.com/?c=d\r\n"),
        "{dropped}"
    );

    let unescaped = request_body(
        &config,
        "GET /unescape?r=http%3A%2F%2Fexample.com%2F%3Ffrom HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        unescaped.contains("Location: http://example.com/?from\r\n"),
        "{unescaped}"
    );

    let escaped_controls = request_body(
        &config,
        "GET /unescape?r=http%3A%2F%2Fexample.com%0D%0Asplit HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        escaped_controls.contains("Location: http://example.com%0D%0Asplit\r\n"),
        "{escaped_controls}"
    );

    let capture = request_body(
        &config,
        "GET /capturedup/%25?a=b HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        capture.ends_with("uri:/capturedup/% args:c=/capturedup/%25&a=b"),
        "{capture}"
    );

    let status = request_body(
        &config,
        "GET /return405 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        status.starts_with("HTTP/1.1 405 Not Allowed\r\n"),
        "{status}"
    );
    assert!(status.contains("<html>"), "{status}");

    let error_handler = request_body(
        &config,
        "GET /error404return405 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        error_handler.starts_with("HTTP/1.1 405 Not Allowed\r\n"),
        "{error_handler}"
    );

    let preserved = request_body(
        &config,
        "GET /error405return200text HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        preserved.starts_with("HTTP/1.1 405 Not Allowed\r\n"),
        "{preserved}"
    );
    assert!(preserved.ends_with("text"), "{preserved}");

    let redirect = request_body(
        &config,
        "GET /error302return200text HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        redirect.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"),
        "{redirect}"
    );
    assert!(redirect.contains("Location: text\r\n"), "{redirect}");
    assert!(redirect.ends_with("text"), "{redirect}");

    let redirected_handler = request_body(
        &config,
        "GET /error405return302 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        redirected_handler.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"),
        "{redirected_handler}"
    );
    assert!(
        redirected_handler.contains("Location: text\r\n"),
        "{redirected_handler}"
    );

    let auto_redirect = request_body(
        &config,
        "GET /auto HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        auto_redirect.starts_with("HTTP/1.1 301 Moved Permanently\r\n"),
        "{auto_redirect}"
    );
    assert!(
        auto_redirect.contains("Location: http://localhost:18151/auto/\r\n"),
        "{auto_redirect}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
