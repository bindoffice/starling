use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn serves_business_routing_with_rewrites_conditionals_and_error_pages() {
    let port = reserve_port();
    let directory = TestDirectory::new("business-routing");
    directory.write("html/app/asset.txt", "rewritten-asset");
    let server = RunningServer::start(directory, port, &site_config(port));

    let rewritten = server.get("/legacy/asset.txt");
    assert_status(&rewritten, 200);
    assert!(
        rewritten.contains("rewritten-asset"),
        "server rewrite did not restart location lookup:\n{rewritten}"
    );

    for (target, expected) in [
        ("/app/missing", "base-error"),
        ("/app/missing?mode=conditional", "conditional-error"),
        ("/app/missing?mode=conditional&reset=yes", "base-error"),
    ] {
        let response = server.get(target);
        assert_status(&response, 404);
        assert!(
            response.contains(expected),
            "{target}: expected {expected}, got:\n{response}"
        );
    }
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn runs_auth_mirror_and_post_action_internal_subrequests() {
    let port = reserve_port();
    let (upstream_port, requests) = subrequest_upstream();
    let directory = TestDirectory::new("business-subrequests");
    let server = RunningServer::start(directory, port, &subrequest_config(port, upstream_port));

    let allowed = server.get("/private/ok?allow=yes");
    assert_status(&allowed, 200);
    assert!(
        allowed.ends_with("upstream-main"),
        "authorized request body missing:\n{allowed}"
    );
    let captured = collect_subrequest_heads(requests);
    for (kind, prefix) in [
        ("main", "GET /upstream-mainok?allow=yes HTTP/1.1\r\n"),
        ("mirror", "GET /mirror?allow=yes HTTP/1.1\r\n"),
        ("post_action", "GET /after HTTP/1.1\r\n"),
    ] {
        assert_eq!(
            captured
                .iter()
                .filter(|request| request.starts_with(prefix))
                .count(),
            1,
            "expected exactly one {kind} upstream request:\n{captured:#?}"
        );
    }

    let denied = server.get("/private/no");
    assert_status(&denied, 403);
    assert!(
        !denied.contains("upstream-main"),
        "denied request reached the content handler:\n{denied}"
    );
}

fn subrequest_upstream() -> (u16, Receiver<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind subrequest upstream");
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept subrequest");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request_head(&mut stream);
            sender.send(request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: 13\r\n\
                 Connection: close\r\n\
                 \r\nupstream-main"
            )
            .unwrap();
        }
    });
    (port, receiver)
}

fn read_request_head(stream: &mut impl Read) -> String {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        let count = stream.read(&mut chunk).unwrap();
        assert!(count > 0, "subrequest closed before its header");
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(request).unwrap();
        }
    }
    panic!("subrequest header exceeded 64 KiB");
}

fn collect_subrequest_heads(receiver: Receiver<String>) -> Vec<String> {
    let mut requests = Vec::new();
    for _ in 0..3 {
        requests.push(
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("subrequest capture timed out"),
        );
    }
    requests
}

fn site_config(port: u16) -> String {
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
                    error_page 404 /errors/base.txt;
                    rewrite ^/legacy/(.*)$ /app/$1 last;

                    location /app/ {{
                        if ($arg_mode = conditional) {{
                            error_page 404 /errors/conditional.txt;
                        }}
                        if ($arg_reset = yes) {{
                        }}
                        try_files $uri =404;
                    }}

                    location = /errors/base.txt {{
                        internal;
                        return 200 "base-error";
                    }}

                    location = /errors/conditional.txt {{
                        internal;
                        return 200 "conditional-error";
                    }}
                }}
            }}
        "#
    )
}

fn subrequest_config(port: u16, upstream_port: u16) -> String {
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
                log_format subrequest '$request_method $request_uri status=$status';
                access_log logs/access.log subrequest;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /private/ {{
                        auth_request /auth?allow=$arg_allow;
                        mirror /mirror;
                        post_action /after;
                        proxy_pass http://127.0.0.1:{upstream_port}/upstream-main;
                    }}

                    location = /auth {{
                        internal;
                        if ($arg_allow = yes) {{
                            return 204;
                        }}
                        return 403;
                    }}

                    location = /mirror {{
                        internal;
                        proxy_pass http://127.0.0.1:{upstream_port};
                    }}

                    location = /after {{
                        internal;
                        proxy_pass http://127.0.0.1:{upstream_port};
                    }}
                }}
            }}
        "#
    )
}
