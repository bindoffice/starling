use super::support::*;
use super::*;

#[tokio::test]
async fn exposes_and_optionally_passes_upstream_trailers() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Transfer-Encoding: chunked\r\n\
                      Connection: close\r\n\r\n\
                      5\r\nhello\r\n\
                      0\r\n\
                      X-Backend-Trailer: done\r\n\
                      X-Hidden-Trailer: secret\r\n\r\n",
                )
                .await
                .unwrap();
        }
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    log_format trailers
                        '$upstream_trailer_x_backend_trailer|$upstream_trailer_x_hidden_trailer|$sent_trailer_x_backend_trailer|$sent_trailer_x_sent';
                    server {{
                        listen 127.0.0.1:18495;
                        location /pass {{
                            access_log logs/pass.log trailers;
                            proxy_pass http://{upstream_address};
                            proxy_hide_header X-Hidden-Trailer;
                            proxy_pass_trailers on;
                            add_trailer X-Sent $upstream_trailer_x_backend_trailer always;
                        }}
                        location /drop {{
                            access_log logs/drop.log trailers;
                            proxy_pass http://{upstream_address};
                            add_trailer X-Sent $upstream_trailer_x_backend_trailer always;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let passed = request_body(
        &config,
        "GET /pass HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        passed.contains("Transfer-Encoding: chunked\r\n"),
        "{passed}"
    );
    assert!(passed.contains("X-Backend-Trailer: done\r\n"), "{passed}");
    assert!(passed.contains("X-Sent: done\r\n"), "{passed}");
    assert!(!passed.contains("X-Hidden-Trailer:"), "{passed}");

    let dropped = request_body(
        &config,
        "GET /drop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        dropped.contains("Transfer-Encoding: chunked\r\n"),
        "{dropped}"
    );
    assert!(!dropped.contains("X-Backend-Trailer:"), "{dropped}");
    assert!(dropped.contains("X-Sent: done\r\n"), "{dropped}");

    let passed_log = std::fs::read_to_string(prefix.join("logs/pass.log")).unwrap_or_default();
    assert_eq!(passed_log.trim_end(), "done|secret|done|done");
    let dropped_log = std::fs::read_to_string(prefix.join("logs/drop.log")).unwrap_or_default();
    assert_eq!(dropped_log.trim_end(), "done|secret|-|done");

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
