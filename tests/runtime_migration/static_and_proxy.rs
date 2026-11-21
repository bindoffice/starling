use super::support::{RunningServer, TestDirectory, assert_status, fake_upstream, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn serves_try_files_and_rewrites_a_real_proxy_location() {
    let port = reserve_port();
    let Some(upstream) = fake_upstream("proxied-through-starling") else {
        return;
    };
    let directory = TestDirectory::new("static-proxy");
    directory.write("html/index.html", "starling-index");
    directory.write("html/assets/site.txt", "starling-asset");
    directory.write("html/errors/404.html", "migrated-error-page");
    let config = site_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let fallback = server.get("/missing/page");
    assert_status(&fallback, 200);
    assert!(
        fallback.contains("starling-index"),
        "try_files fallback failed:\n{fallback}"
    );

    let asset = server.get("/assets/site.txt");
    assert_status(&asset, 200);
    assert!(
        asset.contains("starling-asset"),
        "static asset lookup failed:\n{asset}"
    );

    let error_page = server.get("/strict/missing");
    assert_status(&error_page, 404);
    assert!(
        error_page.contains("migrated-error-page"),
        "error_page internal redirect failed:\n{error_page}"
    );

    let proxied = server.get("/api/items?x=1");
    assert_status(&proxied, 200);
    assert!(
        proxied.contains("proxied-through-starling"),
        "proxy response body missing:\n{proxied}"
    );
    assert!(
        proxied.to_ascii_lowercase().contains("x-upstream: fake"),
        "proxy response headers missing:\n{proxied}"
    );

    let captured = upstream.captured();
    assert!(
        captured.starts_with("GET /v1/items?x=1 HTTP/1.1\r\n"),
        "proxy_pass URI rewrite failed:\n{captured}"
    );
    assert!(
        captured
            .to_ascii_lowercase()
            .contains("x-forwarded-prefix: /api/\r\n"),
        "proxy_set_header was not applied:\n{captured}"
    );
}

fn site_config(port: u16, upstream_port: u16) -> String {
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

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;
                    error_page 404 /errors/404.html;

                    location / {{
                        try_files $uri $uri/ /index.html;
                    }}

                    location /strict/ {{
                        try_files $uri =404;
                    }}

                    location = /errors/404.html {{
                        internal;
                    }}

                    location /api/ {{
                        proxy_pass http://127.0.0.1:{upstream_port}/v1/;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header X-Forwarded-Prefix /api/;
                    }}
                }}
            }}
        "#
    )
}
