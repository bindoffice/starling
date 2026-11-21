use crate::config::BrowserKind;

use super::support::*;
use super::*;

#[test]
fn compiles_browser_rules_with_scope_inheritance() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                modern_browser msie 8.0;
                modern_browser_value modern-http;
                ancient_browser old-bot;
                ancient_browser_value ancient-http;
                server {
                    listen 127.0.0.1:18099;
                    location /inherit { return 200 "ok\n"; }
                    location /replace {
                        modern_browser safari 5.0;
                        modern_browser_value modern-location;
                        ancient_browser netscape4;
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    assert_eq!(server.context.browser.modern.len(), 1);
    assert_eq!(server.context.browser.modern[0].browser, BrowserKind::Msie);
    assert_eq!(server.context.browser.modern_value, "modern-http");
    assert_eq!(server.context.browser.ancient, vec!["old-bot"]);
    assert_eq!(server.context.browser.ancient_value, "ancient-http");

    let inherited = browser_location(server, "/inherit");
    assert_eq!(inherited.context.browser.modern.len(), 1);
    assert_eq!(
        inherited.context.browser.modern[0].browser,
        BrowserKind::Msie
    );

    let replaced = browser_location(server, "/replace");
    assert_eq!(replaced.context.browser.modern.len(), 1);
    assert_eq!(
        replaced.context.browser.modern[0].browser,
        BrowserKind::Safari
    );
    assert_eq!(replaced.context.browser.modern_value, "modern-location");
    assert!(replaced.context.browser.ancient.is_empty());
    assert!(replaced.context.browser.netscape4);
    assert_eq!(replaced.context.browser.ancient_value, "ancient-http");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_browser_configuration() {
    for body in [
        "modern_browser unknown 1.0;",
        "modern_browser msie 5000.0;",
        "modern_browser msie;",
        "ancient_browser;",
    ] {
        let error = test_config_error(&format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18099;
                        location / {{ {body} return 200 "ok\n"; }}
                    }}
                }}
            "#
        ));
        assert!(
            error.contains("browser"),
            "expected browser configuration error, got: {error}"
        );
    }
}

#[tokio::test]
async fn classifies_browsers_and_exposes_variables() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18099;
                    server_name example.test;
                    modern_browser msie 8.0;
                    modern_browser safari 5.0;
                    modern_browser opera 9.0;
                    modern_browser gecko 1.9;
                    modern_browser konqueror 4.0;
                    modern_browser unlisted;
                    modern_browser_value modern;
                    ancient_browser netscape4 AncientBot;
                    ancient_browser_value ancient;
                    location / {
                        return 200 "$modern_browser|$ancient_browser|$msie\n";
                    }
                }
            }
        "#,
        &[],
    );

    for (user_agent, expected) in [
        (None, "modern||\n"),
        (
            Some("Mozilla/5.0 (X11; Linux x86_64) Chrome/120.0"),
            "modern||\n",
        ),
        (
            Some("Mozilla/5.0 (X11; Linux x86_64; rv:20.0) Gecko/20100101 Firefox/20.0"),
            "modern||\n",
        ),
        (
            Some("Mozilla/4.0 (compatible; MSIE 9.0; Windows NT 6.1)"),
            "modern||1\n",
        ),
        (
            Some("Mozilla/4.0 (compatible; MSIE 7.0; Windows NT 6.0)"),
            "|ancient|1\n",
        ),
        (
            Some("Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1) Opera 8.0"),
            "|ancient|\n",
        ),
        (
            Some("Mozilla/5.0 (Macintosh) AppleWebKit/533 Safari/6.0"),
            "modern||\n",
        ),
        (Some("Mozilla/4.72 [en] AncientBot"), "|ancient|\n"),
        (Some("Mozilla/4.72 [en]"), "|ancient|\n"),
        (Some("Unlisted/1.0"), "modern||\n"),
    ] {
        let response = request_bytes(&config, &browser_request(user_agent)).await;
        let (_, body) = split_http_response(&response);
        assert_eq!(
            std::str::from_utf8(body).unwrap(),
            expected,
            "user agent {user_agent:?}"
        );
    }
    let _ = std::fs::remove_dir_all(prefix);
}

fn browser_location<'a>(server: &'a ServerConfig, path: &str) -> &'a LocationConfig {
    server
        .locations
        .iter()
        .find(
            |location| matches!(&location.matcher, LocationMatcher::Prefix(value) if value == path),
        )
        .unwrap()
}

fn browser_request(user_agent: Option<&str>) -> String {
    let mut request = String::from("GET / HTTP/1.1\r\nHost: example.test\r\n");
    if let Some(user_agent) = user_agent {
        request.push_str("User-Agent: ");
        request.push_str(user_agent);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    request
}
