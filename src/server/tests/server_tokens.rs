use super::support::*;

#[tokio::test]
async fn applies_server_tokens_to_headers_and_builtin_error_pages() {
    let (prefix, config) = test_config(
        r#"
            events { worker_connections 64; }
            http {
                server_tokens build;
                server {
                    listen 127.0.0.1:18567;
                    server_name default;
                    server_tokens off;
                    location /inherited {}
                    location /on { server_tokens on; }
                    location /build { server_tokens build; }
                }
                server {
                    listen 127.0.0.1:18567;
                    server_name build.test;
                    location / {}
                }
            }
        "#,
        &[],
    );

    let request = |host: &str, path: &str| {
        format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
    };
    let off = request_body(&config, &request("default", "/inherited")).await;
    assert_header(&off, "Server: nginx");
    assert!(off.contains("<hr><center>nginx</center>"), "{off}");
    assert!(!off.contains("nginx/1.31.9"), "{off}");

    let on = request_body(&config, &request("default", "/on")).await;
    assert_header(&on, "Server: nginx/1.31.9");
    assert!(on.contains("<hr><center>nginx/1.31.9</center>"), "{on}");

    let build = request_body(&config, &request("default", "/build")).await;
    assert_header(&build, &format!("Server: {}", crate::SERVER_SOFTWARE_BUILD));
    assert!(
        build.contains(&format!(
            "<hr><center>{}</center>",
            crate::SERVER_SOFTWARE_BUILD
        )),
        "{build}"
    );

    let inherited = request_body(&config, &request("build.test", "/")).await;
    assert_header(
        &inherited,
        &format!("Server: {}", crate::SERVER_SOFTWARE_BUILD),
    );

    let _ = std::fs::remove_dir_all(prefix);
}

fn assert_header(response: &str, expected: &str) {
    assert!(
        response
            .lines()
            .any(|line| line.eq_ignore_ascii_case(expected)),
        "missing {expected:?} in {response}"
    );
}
