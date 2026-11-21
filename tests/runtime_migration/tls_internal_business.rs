use std::fs;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{
    RunningServer, TestDirectory, assert_status, read_request_head, reserve_port,
    tls_request_unverified,
};

const UPSTREAM_BODY: &str = "upstream-body";
const FILTERED_BODY: &str = "filtered-body";
const DENIED_BODY: &str = "denied-body";

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn tls_rewrite_auth_mirror_filter_cache_store_and_denied_redirect() {
    let port = reserve_port();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let Some(upstream) = BusinessUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("tls-internal-business");
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

    let first = tls_request_unverified(
        port,
        "named.test",
        "GET",
        "/entry/item.txt?allow=yes",
        &[],
        b"",
    );
    assert_status(&first, 200);
    assert!(first.ends_with(FILTERED_BODY), "{first}");
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "the first authorized request should fill the cache:\n{first}"
    );
    assert!(
        first.contains("\r\nX-Business-Hop: guarded\r\n"),
        "the final rewritten location headers were lost:\n{first}"
    );
    assert!(
        !first.contains(UPSTREAM_BODY),
        "the response filter did not replace the upstream body:\n{first}"
    );

    let second = tls_request_unverified(
        port,
        "named.test",
        "GET",
        "/entry/item.txt?allow=yes",
        &[],
        b"",
    );
    assert_status(&second, 200);
    assert!(second.ends_with(FILTERED_BODY), "{second}");
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "the second authorized request should hit the cache:\n{second}"
    );
    assert!(
        second.contains("\r\nX-Business-Hop: guarded\r\n"),
        "the cached response lost the final location headers:\n{second}"
    );

    let denied = tls_request_unverified(
        port,
        "named.test",
        "GET",
        "/entry/item.txt?allow=no",
        &[],
        b"",
    );
    assert_status(&denied, 403);
    assert!(denied.ends_with(DENIED_BODY), "{denied}");
    assert!(
        denied.contains("\r\nX-Business-Hop: denied\r\n"),
        "the denied internal redirect did not use its final headers:\n{denied}"
    );
    assert!(
        denied.contains("\r\nX-Auth-Denied: yes\r\n"),
        "the denied internal redirect was not applied:\n{denied}"
    );
    assert!(
        !denied.contains(UPSTREAM_BODY) && !denied.contains(FILTERED_BODY),
        "the denied request reached the content response path:\n{denied}"
    );

    let requests = upstream.collect(3);
    assert_eq!(
        requests
            .iter()
            .filter(|request| {
                request.starts_with("GET /assets/item.txt?allow=yes HTTP/1.1\r\n")
            })
            .count(),
        1,
        "the cache should coalesce the authorized content fetch:\n{requests:#?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /mirror?allow=yes HTTP/1.1\r\n"))
            .count(),
        2,
        "mirror should run for MISS and HIT but not for the denied request:\n{requests:#?}"
    );

    let stored = server.directory_path().join("html/assets/item.txt");
    wait_until(Duration::from_secs(5), "proxy_store publication", || {
        fs::read(&stored).is_ok_and(|contents| contents == UPSTREAM_BODY.as_bytes())
    });
    assert!(
        !server.directory_path().join("html/denied").exists(),
        "the denied request published a store target"
    );
    server.assert_running();
    upstream.stop();
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
                proxy_cache_path cache levels=1:2 keys_zone=tls_internal:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port} ssl default_server;
                    server_name named.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2 TLSv1.3;
                    root html;
                    error_page 403 = @denied;

                    location /entry/ {{
                        rewrite ^/entry/(.*)$ /assets/$1 last;
                    }}

                    location /assets/ {{
                        auth_request /auth?allow=$arg_allow;
                        mirror /mirror;
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_cache tls_internal;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        sub_filter_types text/plain;
                        sub_filter "{UPSTREAM_BODY}" "{FILTERED_BODY}";
                        add_header X-Cache-Status $upstream_cache_status always;
                        add_header X-Business-Hop guarded always;
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

                    location @denied {{
                        add_header X-Business-Hop denied always;
                        add_header X-Auth-Denied yes always;
                        return 403 "{DENIED_BODY}";
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
    )
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

struct BusinessUpstream {
    port: u16,
    requests: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl BusinessUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping TLS internal business test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind TLS business upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (sender, requests) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = thread::spawn({
            let stop = stop.clone();
            move || accept_loop(listener, sender, stop)
        });
        Some(Self {
            port,
            requests,
            stop,
            accept: Some(accept),
        })
    }

    fn collect(&self, count: usize) -> Vec<String> {
        (0..count)
            .map(|_| {
                self.requests
                    .recv_timeout(Duration::from_secs(5))
                    .expect("TLS business upstream request timed out")
            })
            .collect()
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(listener: TcpListener, requests: mpsc::Sender<String>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let requests = requests.clone();
                thread::spawn(move || serve_connection(stream, requests));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("TLS business upstream accept failed: {error}"),
        }
    }
}

fn serve_connection(mut stream: TcpStream, requests: mpsc::Sender<String>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let Ok(request) = read_request_head(&mut stream) else {
        return;
    };
    let mirror = request.starts_with("GET /mirror");
    if requests.send(request).is_err() {
        return;
    }
    let response = if mirror {
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{UPSTREAM_BODY}",
            UPSTREAM_BODY.len()
        )
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
