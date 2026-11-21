use super::support::*;

const MSIE: &str = "Mozilla/4.0 (compatible; MSIE 8.0; Windows NT 6.1)";
const CHROME: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120.0 Safari/537.36";
const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0";

#[tokio::test]
async fn pads_builtin_error_pages_for_msie_and_chrome() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18523;
                    root html;
                    location /default { try_files $uri =404; }
                    location /disabled {
                        msie_padding off;
                        try_files $uri =404;
                    }
                }
            }
        "#,
        &[],
    );

    for user_agent in [MSIE, CHROME] {
        let response = request_body(
            &config,
            &format!(
                "GET /default/missing HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {user_agent}\r\nConnection: close\r\n\r\n"
            ),
        )
        .await;
        assert_eq!(
            response
                .matches("<!-- a padding to disable MSIE and Chrome friendly error page -->")
                .count(),
            6,
            "{response}"
        );
    }

    let firefox = request_body(
        &config,
        &format!(
            "GET /default/missing HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {FIREFOX}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(!firefox.contains("a padding to disable MSIE"));

    let disabled = request_body(
        &config,
        &format!(
            "GET /disabled/missing HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {CHROME}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(!disabled.contains("a padding to disable MSIE"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rewrites_redirects_for_msie_when_refresh_is_enabled() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18524;
                    location /refresh {
                        msie_refresh on;
                        return 302 /target?x=1;
                    }
                    location /default {
                        return 302 /target;
                    }
                }
            }
        "#,
        &[],
    );

    let refreshed = request_body(
        &config,
        &format!(
            "GET /refresh HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {MSIE}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(refreshed.starts_with("HTTP/1.1 200 OK\r\n"), "{refreshed}");
    assert!(!refreshed.contains("Location:"), "{refreshed}");
    assert!(
        refreshed.contains("Content-Type: text/html\r\n"),
        "{refreshed}"
    );
    assert!(
        refreshed.ends_with(
            "<html><head><meta http-equiv=\"Refresh\" content=\"0; URL=/target?x=1\"></head><body></body></html>\r\n"
        ),
        "{refreshed}"
    );

    let chrome = request_body(
        &config,
        &format!(
            "GET /refresh HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {CHROME}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(chrome.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(chrome.contains("Location: http://localhost:18524/target?x=1\r\n"));

    let default = request_body(
        &config,
        &format!(
            "GET /default HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {MSIE}\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert!(default.starts_with("HTTP/1.1 302 Moved Temporarily\r\n"));
    assert!(default.contains("Location: http://localhost:18524/target\r\n"));
    let _ = std::fs::remove_dir_all(prefix);
}
