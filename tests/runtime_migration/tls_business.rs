use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::support::{
    RunningServer, TestDirectory, assert_status, fake_upstream, reserve_port,
    tls_request_unverified,
};

const UPSTREAM_BODY: &str = "sni-business-body";

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn routes_sni_rewrite_cache_store_and_limit_except_over_tls() {
    let port = reserve_port();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let Some(upstream) = fake_upstream(UPSTREAM_BODY) else {
        return;
    };
    let directory = TestDirectory::new("tls-business");
    let mut server = RunningServer::start(
        directory,
        port,
        &config(
            port,
            upstream.port,
            &fixtures.join("test.crt"),
            &fixtures.join("test.key"),
        ),
    );

    let default = tls_request_unverified(port, "default.test", "GET", "/", &[], b"");
    assert_status(&default, 200);
    assert!(default.ends_with("default:GET"), "{default}");

    let first = tls_request_unverified(port, "named.test", "GET", "/legacy/item.txt", &[], b"");
    assert_status(&first, 200);
    assert!(first.ends_with(UPSTREAM_BODY), "{first}");
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "the rewritten request should fill the cache:\n{first}"
    );
    assert!(
        first.contains("\r\nX-Rewrite-Hop: assets\r\n"),
        "the final rewritten location should supply its response headers:\n{first}"
    );

    let second = tls_request_unverified(port, "named.test", "GET", "/legacy/item.txt", &[], b"");
    assert_status(&second, 200);
    assert!(second.ends_with(UPSTREAM_BODY), "{second}");
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "the second rewritten request should hit the cache:\n{second}"
    );

    let denied = tls_request_unverified(
        port,
        "named.test",
        "POST",
        "/legacy/item.txt",
        &[],
        b"denied",
    );
    assert_status(&denied, 403);
    assert!(
        !denied.contains(UPSTREAM_BODY),
        "limit_except denial reached the content upstream:\n{denied}"
    );

    let captured = upstream.captured();
    assert!(
        captured.starts_with("GET /assets/item.txt HTTP/1.1\r\n"),
        "the rewrite should reach the upstream once with the final URI:\n{captured}"
    );
    let stored = server.directory_path().join("html/assets/item.txt");
    wait_until(Duration::from_secs(5), "proxy_store publication", || {
        fs::read(&stored).is_ok_and(|contents| contents == UPSTREAM_BODY.as_bytes())
    });
    server.assert_running();
}

fn config(port: u16, upstream_port: u16, certificate: &Path, key: &Path) -> String {
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
                proxy_cache_path cache levels=1:2 keys_zone=tls_cache:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port} ssl default_server;
                    server_name default.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2 TLSv1.3;

                    location / {{
                        return 200 "default:$request_method";
                    }}
                }}

                server {{
                    listen 127.0.0.1:{port} ssl;
                    server_name named.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2 TLSv1.3;
                    root html;

                    location /legacy/ {{
                        rewrite ^/legacy/(.*)$ /assets/$1 last;
                    }}

                    location /assets/ {{
                        limit_except GET {{
                            deny all;
                        }}
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_cache tls_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        add_header X-Cache-Status $upstream_cache_status always;
                        add_header X-Rewrite-Hop assets always;
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
        certificate.display(),
        key.display()
    )
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
