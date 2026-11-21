use super::support::*;

#[test]
fn follows_recursive_error_pages_only_when_enabled() {
    run_with_server_stack(async {
        let (prefix, config) = test_config(
            r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18520 default_server;
                    server_name disabled.test;
                    root html;
                    error_page 404 /hop;
                    location = /hop {
                        internal;
                        recursive_error_pages on;
                        error_page 404 /final;
                        try_files $uri =404;
                    }
                    location = /final { internal; }
                    location / { try_files $uri =404; }
                }
                server {
                    listen 127.0.0.1:18520;
                    server_name enabled.test;
                    root html;
                    recursive_error_pages on;
                    error_page 404 /hop;
                    location = /hop {
                        internal;
                        error_page 404 /final;
                        try_files $uri =404;
                    }
                    location = /final { internal; }
                    location / { try_files $uri =404; }
                }
            }
        "#,
            &[("html/final", "redirected page\n")],
        );

        let disabled = request_body(
            &config,
            "GET /missing HTTP/1.1\r\nHost: disabled.test\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(disabled.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(disabled.contains("<center><h1>404 Not Found</h1></center>"));
        assert!(!disabled.contains("redirected page"));

        let enabled = request_body(
            &config,
            "GET /missing HTTP/1.1\r\nHost: enabled.test\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(enabled.starts_with("HTTP/1.1 404 Not Found\r\n"));
        assert!(enabled.ends_with("redirected page\n"));
        let _ = std::fs::remove_dir_all(prefix);
    });
}

fn run_with_server_stack(future: impl std::future::Future<Output = ()> + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(crate::runtime::WORKER_THREAD_STACK_SIZE)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[tokio::test]
async fn location_error_pages_replace_server_inheritance() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18521;
                    root html;
                    error_page 404 /server-fallback;
                    location /replaced {
                        error_page 500 /server-fallback;
                        try_files $uri =404;
                    }
                    location = /server-fallback { internal; }
                    location / { try_files $uri =404; }
                }
            }
        "#,
        &[("html/server-fallback", "server fallback\n")],
    );

    let response = request_body(
        &config,
        "GET /replaced HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(response.contains("<center><h1>404 Not Found</h1></center>"));
    assert!(!response.contains("server fallback"));

    let inherited = request_body(
        &config,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(inherited.starts_with("HTTP/1.1 404 Not Found\r\n"));
    assert!(inherited.ends_with("server fallback\n"));
    let _ = std::fs::remove_dir_all(prefix);
}
