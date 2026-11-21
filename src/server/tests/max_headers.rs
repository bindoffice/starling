use super::support::*;

#[tokio::test]
async fn enforces_http1_max_headers() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18564;
                    max_headers 3;
                    location / {
                        return 200 "ok\n";
                    }
                }
            }
        "#,
        &[],
    );
    let request = |extra: &str| {
        format!("GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra}\r\n")
    };

    let accepted = request_body(&config, &request("X-One: 1\r\n")).await;
    assert!(accepted.starts_with("HTTP/1.1 200 OK\r\n"), "{accepted}");
    assert!(accepted.ends_with("ok\n"));

    let rejected = request_body(&config, &request("X-One: 1\r\nX-Two: 2\r\n")).await;
    assert!(
        rejected.starts_with("HTTP/1.1 400 Bad Request\r\n"),
        "{rejected}"
    );
    assert!(rejected.contains("Connection: close\r\n"), "{rejected}");

    let _ = std::fs::remove_dir_all(prefix);
}
