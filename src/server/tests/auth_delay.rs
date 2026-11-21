use super::support::*;
use super::*;

#[tokio::test]
async fn delays_unauthorized_responses_with_auth_delay() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                auth_basic "Restricted";
                auth_basic_user_file conf/htpasswd;
                auth_delay 40ms;
                server {
                    listen 127.0.0.1:18211;
                    location / { return 200 "ok\n"; }
                }
            }
        "#,
        &[("conf/htpasswd", "user:{PLAIN}pass\n")],
    );

    let started = Instant::now();
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
        "{response}"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(30),
        "auth_delay was not applied: {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic dXNlcjpwYXNz\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        started.elapsed() < Duration::from_millis(30),
        "auth_delay should only affect unauthorized responses: {:?}",
        started.elapsed()
    );
    let _ = std::fs::remove_dir_all(prefix);
}
