use super::support::*;

#[tokio::test]
async fn honors_request_version_and_user_agent_gzip_rules() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                gzip on;
                gzip_types text/plain;
                gzip_min_length 1;
                gzip_disable "mozilla" msie6;
                server {
                    listen 127.0.0.1:18512;
                    location /default {
                        return 200 "abcdefghijklmnopqrstuvwxyz";
                    }
                    location /version10 {
                        gzip_http_version 1.0;
                        return 200 "abcdefghijklmnopqrstuvwxyz";
                    }
                    location /allow {
                        gzip_disable "does-not-match";
                        return 200 "abcdefghijklmnopqrstuvwxyz";
                    }
                }
            }
        "#,
        &[],
    );

    let compressed = |response: &[u8]| {
        let (headers, _) = split_http_response(response);
        headers.contains("Content-Encoding: gzip\r\n")
    };

    let response = request_bytes(
        &config,
        "GET /default HTTP/1.0\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!compressed(&response), "HTTP/1.0 must not be compressed");

    let response = request_bytes(
        &config,
        "GET /version10 HTTP/1.0\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        compressed(&response),
        "gzip_http_version 1.0 must allow HTTP/1.0"
    );

    let response = request_bytes(
        &config,
        "GET /default HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nUser-Agent: MOZILLA/5.0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        !compressed(&response),
        "gzip_disable regex must be case-insensitive"
    );

    let response = request_bytes(
        &config,
        "GET /allow HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nUser-Agent: MOZILLA/5.0\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        compressed(&response),
        "location gzip_disable must replace inherited regexes"
    );

    let response = request_bytes(
        &config,
        "GET /default HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nUser-Agent: Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1)\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!compressed(&response), "msie6 must disable gzip");

    let response = request_bytes(
        &config,
        "GET /allow HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nUser-Agent: Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1; SV1)\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        compressed(&response),
        "IE6 SV1 must not be treated as legacy msie6"
    );

    let response = request_bytes(
        &config,
        "GET /allow HTTP/1.1\r\nHost: localhost\r\nAccept-Encoding: gzip\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(compressed(&response), "HTTP/1.1 must support gzip");
    let _ = std::fs::remove_dir_all(prefix);
}
