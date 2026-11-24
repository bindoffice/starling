use std::fs;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use super::disk_fault_support::{EIO, FaultServer, FaultSpec, compiler_available};
use super::support::{
    TestDirectory, assert_status, read_request_head, reserve_port, tls_request_unverified,
};

const RAW_BODY: &str = "raw-tls-matrix-body";
const FILTERED_BODY: &str = "filtered-tls-matrix-body";
const DENIED_BODY: &str = "denied-after-multihop";

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn tls_multihop_business_flow_survives_store_failures_and_denied_redirects() {
    if compiler_available().is_none() {
        eprintln!("skipping TLS redirect failure matrix: cc is not available");
        return;
    }

    let port = reserve_port();
    let Some(upstream) = MatrixUpstream::start() else {
        return;
    };
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let directory = TestDirectory::new("tls-redirect-failure-matrix");
    let config = config(
        port,
        upstream.port,
        &fixtures.join("test.crt"),
        &fixtures.join("test.key"),
    );
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &config,
        FaultSpec {
            path: "/html/",
            error: EIO,
            operations: "write,unlink,close",
        },
    ) else {
        return;
    };

    let first = tls_request_unverified(
        port,
        "matrix.test",
        "GET",
        "/entry/item.txt?allow=yes",
        &[],
        b"",
    );
    assert_status(&first, 200);
    assert!(first.ends_with(FILTERED_BODY), "{first}");
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "the first TLS request should fill the cache:\n{first}"
    );
    assert!(
        first.contains("\r\nX-Final-Hop: assets\r\n"),
        "multi-hop rewrite lost the final location headers:\n{first}"
    );
    assert!(
        !first.contains(RAW_BODY),
        "the response filter was bypassed:\n{first}"
    );

    let second = tls_request_unverified(
        port,
        "matrix.test",
        "GET",
        "/entry/item.txt?allow=yes",
        &[],
        b"",
    );
    assert_status(&second, 200);
    assert!(second.ends_with(FILTERED_BODY), "{second}");
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "the second TLS request should hit the cache despite store failure:\n{second}"
    );
    assert!(
        second.contains("\r\nX-Final-Hop: assets\r\n"),
        "cached response lost the final rewritten location headers:\n{second}"
    );

    let denied = tls_request_unverified(
        port,
        "matrix.test",
        "GET",
        "/entry/item.txt?allow=no",
        &[],
        b"",
    );
    if denied.is_empty() {
        server.assert_running();
        panic!(
            "denied TLS request returned no response:\n{}",
            server.diagnostics()
        );
    }
    assert_status(&denied, 403);
    assert!(denied.ends_with(DENIED_BODY), "{denied}");
    assert!(
        denied.contains("\r\nX-Final-Hop: denied\r\n"),
        "multi-hop error page lost its final location headers:\n{denied}"
    );
    assert!(
        !denied.contains(RAW_BODY) && !denied.contains(FILTERED_BODY),
        "the denied TLS request reached the content upstream:\n{denied}"
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
        "cache/store/auth/mirror should contact the content upstream once:\n{requests:#?}"
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /mirror?allow=yes HTTP/1.1\r\n"))
            .count(),
        2,
        "mirror should run for both cache MISS and HIT:\n{requests:#?}"
    );
    assert!(
        !server.temp_path("html/assets/item.txt").exists(),
        "store published a target after injected write and unlink failures"
    );
    assert!(
        !temporary_store_files(&server.temp_path("html")).is_empty(),
        "store cleanup failures should remain observable"
    );
    server.assert_running();
    upstream.stop();
}

struct MatrixUpstream {
    port: u16,
    requests: mpsc::Receiver<String>,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl MatrixUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping TLS redirect failure matrix: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind TLS redirect matrix upstream: {error}"),
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
                    .expect("TLS redirect matrix upstream timed out")
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
            Err(error) => panic!("TLS redirect matrix accept failed: {error}"),
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
        "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_owned()
    } else {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{RAW_BODY}",
            RAW_BODY.len()
        )
    };
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn temporary_store_files(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".starling-store-"))
        })
        .collect()
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn config(port: u16, upstream_port: u16, certificate: &Path, key: &Path) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log stderr notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log off;
                proxy_cache_path cache levels=1:2 keys_zone=tls_redirect:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port} ssl default_server;
                    server_name matrix.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2 TLSv1.3;
                    root html;
                    error_page 403 = /deny-entry;

                    location /entry {{
                        rewrite ^/entry(.*)$ /hop-one$1 last;
                    }}

                    location /hop-one {{
                        internal;
                        rewrite ^/hop-one(.*)$ /assets$1 last;
                    }}

                    location /assets/ {{
                        auth_request /auth?allow=$arg_allow;
                        mirror /mirror;
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_buffering off;
                        proxy_cache tls_redirect;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        sub_filter_types text/plain;
                        sub_filter "{RAW_BODY}" "{FILTERED_BODY}";
                        add_header X-Cache-Status $upstream_cache_status always;
                        add_header X-Final-Hop assets always;
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

                    location = /deny-entry {{
                        internal;
                        rewrite ^ /deny-final last;
                    }}

                    location = /deny-final {{
                        internal;
                        add_header X-Final-Hop denied always;
                        return 403 "{DENIED_BODY}";
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
    )
}
