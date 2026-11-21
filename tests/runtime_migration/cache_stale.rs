use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, fake_upstream, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn serves_stale_cache_when_the_upstream_becomes_unavailable() {
    let port = reserve_port();
    let Some(upstream) = fake_upstream("cache-stale-body") else {
        return;
    };
    let directory = TestDirectory::new("cache-stale");
    let config = stale_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let first = server.get("/stale");
    assert_status(&first, 200);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "first response was not a cache miss:\n{first}"
    );
    assert!(first.contains("cache-stale-body"), "{first}");
    let captured = upstream.captured();
    assert!(
        captured.starts_with("GET /stale HTTP/1.1\r\n"),
        "unexpected upstream request:\n{captured}"
    );

    thread::sleep(Duration::from_millis(100));
    let stale = server.get("/stale");
    assert_status(&stale, 200);
    assert!(
        stale.to_ascii_lowercase().contains("x-cache-status: stale"),
        "expired entry was not served stale after backend failure:\n{stale}"
    );
    assert!(stale.contains("cache-stale-body"), "{stale}");
}

fn stale_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache levels=1:2 keys_zone=runtime_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    add_header X-Cache-Status $upstream_cache_status always;

                    location / {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_cache runtime_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1ms;
                        proxy_cache_use_stale error timeout;
                    }}
                }}
            }}
        "#
    )
}
