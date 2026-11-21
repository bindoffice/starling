use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use super::support::{RunningServer, TestDirectory, assert_status, fake_upstream, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn proxy_cache_and_store_share_the_first_upstream_response() {
    let port = reserve_port();
    let Some(upstream) = fake_upstream("proxy-cache-store-body") else {
        return;
    };
    let directory = TestDirectory::new("proxy-store-cache");
    let prefix = directory.path().to_path_buf();
    let config = proxy_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let first = server.get("/assets/cached.txt?x=1");
    assert_status(&first, 200);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "first proxy response was not a cache miss:\n{first}"
    );
    assert!(
        first.contains("proxy-cache-store-body"),
        "proxy response body missing:\n{first}"
    );

    let captured = upstream.captured();
    assert!(
        captured.starts_with("GET /assets/cached.txt?x=1 HTTP/1.1\r\n"),
        "upstream request URI was not preserved:\n{captured}"
    );

    let stored_path = prefix.join("html/assets/cached.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read(&stored_path).is_ok_and(|contents| contents == b"proxy-cache-store-body") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "proxy_store did not publish {}:\n{}",
            stored_path.display(),
            first
        );
        thread::sleep(Duration::from_millis(20));
    }

    let second = server.get("/assets/cached.txt?x=1");
    assert_status(&second, 200);
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "second proxy response was not a cache hit:\n{second}"
    );
    assert!(
        second.contains("proxy-cache-store-body"),
        "cached proxy response body missing:\n{second}"
    );
}

fn proxy_config(port: u16, upstream_port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log logs/access.log;
                proxy_cache_path cache levels=1:2
                                 keys_zone=runtime_cache:1m
                                 inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location /assets/ {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_cache runtime_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        proxy_store_access user:rw group:r;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}
