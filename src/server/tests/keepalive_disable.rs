use super::support::*;

const MSIE6: &str = "Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1)";
const SAFARI: &str = "Mozilla/5.0 (Macintosh; U; Intel Mac OS X 10_6_8) AppleWebKit/533 Safari/5.0";

fn pipelined_request(host: &str, method: &str, user_agent: &str) -> String {
    let body = if method == "POST" {
        "Content-Length: 0\r\n"
    } else {
        ""
    };
    format!(
        "{method} /{host} HTTP/1.1\r\nHost: localhost\r\nUser-Agent: {user_agent}\r\n\
         {body}\r\nGET /{host} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
}

#[tokio::test]
async fn applies_msie6_and_safari_keepalive_disable_rules() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18516;
                    location /default { return 200 "ok\n"; }
                    location /none {
                        keepalive_disable none;
                        return 200 "ok\n";
                    }
                    location /safari {
                        keepalive_disable safari;
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );

    let msie_post = request_body(&config, &pipelined_request("default", "POST", MSIE6)).await;
    assert_eq!(msie_post.matches("HTTP/1.1 200 OK\r\n").count(), 1);
    assert!(msie_post.contains("Connection: close\r\n"), "{msie_post}");

    let msie_get = request_body(&config, &pipelined_request("default", "GET", MSIE6)).await;
    assert_eq!(msie_get.matches("HTTP/1.1 200 OK\r\n").count(), 2);

    let msie_none = request_body(&config, &pipelined_request("none", "POST", MSIE6)).await;
    assert_eq!(msie_none.matches("HTTP/1.1 200 OK\r\n").count(), 2);

    let safari_default = request_body(&config, &pipelined_request("default", "GET", SAFARI)).await;
    assert_eq!(safari_default.matches("HTTP/1.1 200 OK\r\n").count(), 2);

    let safari_disabled = request_body(&config, &pipelined_request("safari", "GET", SAFARI)).await;
    assert_eq!(safari_disabled.matches("HTTP/1.1 200 OK\r\n").count(), 1);
    assert!(
        safari_disabled.contains("Connection: close\r\n"),
        "{safari_disabled}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
