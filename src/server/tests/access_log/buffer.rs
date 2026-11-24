use super::super::support::*;
use crate::logging::flush_access_logs;
use flate2::read::MultiGzDecoder;
use std::io::Read;
use std::time::Duration;

#[tokio::test]
async fn flushes_buffered_http_access_logs_on_timer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18190;
                    access_log logs/buffered.log status buffer=4k flush=20ms;
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
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/buffered.log")).unwrap(),
        "200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn gzip_compresses_flushed_http_access_logs() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                log_format status '$status';
                server {
                    listen 127.0.0.1:18191;
                    access_log logs/buffered.log status buffer=4k gzip=6;
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
    flush_access_logs();
    let compressed = std::fs::read(prefix.join("logs/buffered.log")).unwrap();
    let mut decoder = MultiGzDecoder::new(compressed.as_slice());
    let mut contents = String::new();
    decoder.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "200\n");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn flushes_buffered_stream_access_logs_on_timer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                log_format session '$server_name';
                access_log logs/buffered.log session buffer=4k flush=20ms;
                server {
                    listen 127.0.0.1:13321;
                    server_name buffered.test;
                    return "ok";
                }
            }
        "#,
        &[],
    );

    assert_eq!(stream_response(&config, b"").await, b"ok");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/buffered.log")).unwrap(),
        "buffered.test\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
