use super::support::*;

#[tokio::test]
async fn disables_chunked_trailers_when_transfer_encoding_is_off() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                chunked_transfer_encoding off;
                add_trailer X-Trailer inherited always;
                server {
                    listen 127.0.0.1:18514;
                    location /inherit {
                        return 200 "hello\n";
                    }
                    location /enabled {
                        chunked_transfer_encoding on;
                        return 200 "hello\n";
                    }
                }
            }
        "#,
        &[],
    );

    let disabled = request_body(
        &config,
        "GET /inherit HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(!disabled.contains("Transfer-Encoding:"), "{disabled}");
    assert!(disabled.contains("Content-Length: 6\r\n"), "{disabled}");
    assert!(!disabled.contains("X-Trailer:"), "{disabled}");
    assert!(disabled.ends_with("\r\n\r\nhello\n"), "{disabled:?}");

    let enabled = request_body(
        &config,
        "GET /enabled HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        enabled.contains("Transfer-Encoding: chunked\r\n"),
        "{enabled}"
    );
    assert!(!enabled.contains("Content-Length:"), "{enabled}");
    assert!(
        enabled.ends_with("6\r\nhello\n\r\n0\r\nX-Trailer: inherited\r\n\r\n"),
        "{enabled:?}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
