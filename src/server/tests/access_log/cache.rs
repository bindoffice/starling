use super::super::support::*;
use crate::logging::{flush_access_logs, reopen_access_logs};

#[tokio::test]
async fn reopens_cached_http_access_log_after_rotation() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                open_log_file_cache max=4 inactive=10s min_uses=1 valid=1h;
                server {
                    listen 127.0.0.1:18292;
                    access_log logs/cached.log status;
                    return 200 "ok\n";
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");

    let log = prefix.join("logs/cached.log");
    let rotated = prefix.join("logs/cached.log.old");
    std::fs::rename(&log, &rotated).unwrap();
    reopen_access_logs();

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");

    assert_eq!(std::fs::read_to_string(&rotated).unwrap(), "200\n");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "200\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reopens_cached_stream_access_log_after_rotation() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                log_format session '$server_name';
                open_log_file_cache max=4;
                access_log logs/cached-stream.log session;
                server {
                    listen 127.0.0.1:13391;
                    server_name cached.test;
                    return "ok";
                }
            }
        "#,
        &[],
    );

    assert_eq!(stream_response(&config, b"").await, b"ok");
    let log = prefix.join("logs/cached-stream.log");
    let rotated = prefix.join("logs/cached-stream.log.old");
    std::fs::rename(&log, &rotated).unwrap();
    reopen_access_logs();

    assert_eq!(stream_response(&config, b"").await, b"ok");
    assert_eq!(std::fs::read_to_string(&rotated).unwrap(), "cached.test\n");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "cached.test\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reopens_cached_buffered_access_log_after_rotation() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                open_log_file_cache max=4;
                server {
                    listen 127.0.0.1:18293;
                    access_log logs/buffered-cached.log status buffer=4k;
                    return 204;
                }
            }
        "#,
        &[],
    );

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{response}"
    );
    flush_access_logs();

    let log = prefix.join("logs/buffered-cached.log");
    let rotated = prefix.join("logs/buffered-cached.log.old");
    std::fs::rename(&log, &rotated).unwrap();
    reopen_access_logs();

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{response}"
    );
    flush_access_logs();

    assert_eq!(std::fs::read_to_string(&rotated).unwrap(), "204\n");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "204\n");
    let _ = std::fs::remove_dir_all(prefix);
}
