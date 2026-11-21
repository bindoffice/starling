use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

const MAIN_BODY: &str = "main-cache-store";

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn cache_store_auth_and_mirror_share_one_cache_fill() {
    let port = reserve_port();
    let (upstream_port, requests) = cache_mirror_upstream();
    let directory = TestDirectory::new("business-cache-matrix");
    let config = cache_mirror_config(port, upstream_port);
    let server = RunningServer::start(directory, port, &config);

    let first = server.get("/assets/cache.txt?allow=yes");
    assert_status(&first, 200);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "first request should fill the cache:\n{first}"
    );
    assert!(first.ends_with(MAIN_BODY), "{first}");

    let second = server.get("/assets/cache.txt?allow=yes");
    assert_status(&second, 200);
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "second authorized request should hit the cache:\n{second}"
    );
    assert!(second.ends_with(MAIN_BODY), "{second}");

    let denied = server.get("/assets/cache.txt?allow=no");
    assert_status(&denied, 403);
    assert!(
        !denied.contains(MAIN_BODY),
        "denied request reached the content upstream:\n{denied}"
    );

    let captured = collect_requests(requests, 3);
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.starts_with("GET /assets/cache.txt?allow=yes HTTP/1.1\r\n"))
            .count(),
        1,
        "cache/store/auth matrix should contact the main upstream once:\n{captured:#?}"
    );
    assert_eq!(
        captured
            .iter()
            .filter(|request| request.starts_with("GET /mirror?allow=yes HTTP/1.1\r\n"))
            .count(),
        2,
        "both authorized requests should be mirrored:\n{captured:#?}"
    );

    let stored = server.directory_path().join("html/assets/cache.txt");
    wait_until(Duration::from_secs(5), "proxy_store publication", || {
        fs::read(&stored).is_ok_and(|contents| contents == MAIN_BODY.as_bytes())
    });
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn error_page_and_try_files_finish_a_multi_hop_internal_redirect() {
    let port = reserve_port();
    let Some((upstream_port, captured)) = error_page_upstream() else {
        return;
    };
    let directory = TestDirectory::new("business-error-matrix");
    let config = error_page_config(port, upstream_port);
    let server = RunningServer::start(directory, port, &config);

    let response = post(
        port,
        "/entry",
        "Host: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    );
    assert_status(&response, 200);
    assert!(response.ends_with("matrix-fallback"), "{response}");
    assert!(
        response.contains("\r\nX-Response-Hop: final\r\n"),
        "response filters should use the final error-page location:\n{response}"
    );
    assert!(
        !response.contains("\r\nX-Response-Hop: middle\r\n"),
        "the triggering location leaked response configuration:\n{response}"
    );
    let request_body_file = response
        .lines()
        .find_map(|line| line.strip_prefix("X-Request-Body-File: "))
        .unwrap_or_else(|| {
            panic!("multi-hop internal redirect should retain the request body file:\n{response}")
        });
    assert!(
        request_body_file.contains(".starling-tmp-"),
        "unexpected request body path: {request_body_file}"
    );

    let request = captured
        .recv_timeout(Duration::from_secs(5))
        .expect("error-page upstream capture timed out");
    assert!(
        request.starts_with("POST /middle HTTP/1.1\r\n"),
        "internal redirect should reach the upstream with the rewritten URI:\n{request}"
    );
    assert!(request.ends_with("\r\n\r\npartmore"), "{request}");

    let temp_root = server.directory_path().join("body_temp");
    wait_until(Duration::from_secs(5), "request body cleanup", || {
        !contains_generated_temp_file(&temp_root)
    });
}

fn cache_mirror_upstream() -> (u16, Receiver<String>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind cache/mirror upstream");
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().expect("accept cache/mirror request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let request = read_request_head(&mut stream);
            let mirror = request.starts_with("GET /mirror");
            sender.send(request).unwrap();
            if mirror {
                stream
                    .write_all(
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\
                          Connection: close\r\n\r\n",
                    )
                    .unwrap();
            } else {
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{MAIN_BODY}",
                    MAIN_BODY.len()
                )
                .unwrap();
            }
        }
    });
    (port, receiver)
}

fn error_page_upstream() -> Option<(u16, Receiver<String>)> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping business matrix test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind error-page upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept error-page request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = read_request(&mut stream);
        sender.send(request).unwrap();
        stream
            .write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\
                  Content-Length: 7\r\nConnection: close\r\n\r\nmissing",
            )
            .unwrap();
    });
    Some((port, receiver))
}

fn collect_requests(receiver: Receiver<String>, count: usize) -> Vec<String> {
    (0..count)
        .map(|_| {
            receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("business matrix upstream capture timed out")
        })
        .collect()
}

fn post(port: u16, target: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to starling");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(stream, "POST {target} HTTP/1.1\r\n{request}").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

fn read_request_head(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(request).unwrap();
        }
    }
    panic!("upstream request headers exceeded 64 KiB");
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while request.len() < header_end + content_length {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "upstream closed before request body");
        request.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(request).unwrap()
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn contains_generated_temp_file(root: &std::path::Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            contains_generated_temp_file(&path)
        } else {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".starling-tmp-"))
        }
    })
}

fn cache_mirror_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache levels=1:2 keys_zone=runtime_cache:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location /assets/ {{
                        auth_request /auth?allow=$arg_allow;
                        mirror /mirror;
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_cache runtime_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        add_header X-Cache-Status $upstream_cache_status always;
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
                }}
            }}
        "#
    )
}

fn error_page_config(port: u16, upstream_port: u16) -> String {
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
                log_format body_file '$request_body_file status=$status';
                access_log logs/access.log body_file;
                client_body_buffer_size 4;
                client_body_temp_path body_temp;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;
                    error_page 404 = /recover;

                    location = /entry {{
                        rewrite ^ /middle last;
                    }}

                    location = /middle {{
                        internal;
                        proxy_intercept_errors on;
                        proxy_pass http://127.0.0.1:{upstream_port};
                        add_header X-Response-Hop middle always;
                    }}

                    location = /recover {{
                        internal;
                        try_files /missing @final;
                    }}

                    location @final {{
                        add_header X-Response-Hop final always;
                        add_header X-Request-Body-File $request_body_file always;
                        return 200 "matrix-fallback";
                    }}
                }}
            }}
        "#
    )
}
