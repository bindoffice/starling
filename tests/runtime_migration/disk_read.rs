use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::disk_fault_support::{
    EIO, FaultServer, FaultSpec, compiler_available, dav_config, response_config, wait_for_no_files,
};
use super::support::{TestDirectory, fake_upstream, read_request_head, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn read_faults_preserve_committed_or_request_error_semantics() {
    if compiler_available().is_none() {
        eprintln!("skipping disk-read migration: cc is not available");
        return;
    }

    response_spill_read_eio_truncates_after_commit();
    static_read_eio_truncates_after_commit();
    dav_read_eio_truncates_after_commit();
    cache_read_eio_falls_back_to_upstream();
    request_body_read_eio_maps_to_500();
}

fn response_spill_read_eio_truncates_after_commit() {
    let port = reserve_port();
    let body = "r".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("read-response-spill");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        FaultSpec {
            path: "/proxy-temp/",
            error: EIO,
            operations: "read",
        },
    ) else {
        return;
    };

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("read-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "committed response was rewritten after a spill read failure:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "spill read failure did not truncate the response"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("proxy-temp"), "response spill read");
}

fn static_read_eio_truncates_after_commit() {
    let port = reserve_port();
    let body = "s".repeat(128 * 1024);
    let directory = TestDirectory::new("read-static");
    directory.write("html/read.txt", &body);
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &static_config(port),
        FaultSpec {
            path: "/html/",
            error: EIO,
            operations: "read",
        },
    ) else {
        return;
    };

    let response = request(port, "/read.txt");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("static read-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "static read failure rewrote the response:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "static read failure did not truncate the response"
    );
    server.assert_running();
}

fn dav_read_eio_truncates_after_commit() {
    let port = reserve_port();
    let body = "d".repeat(128 * 1024);
    let directory = TestDirectory::new("read-dav");
    directory.write("html/dav/read.bin", &body);
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &dav_config(port),
        FaultSpec {
            path: "/html/",
            error: EIO,
            operations: "read",
        },
    ) else {
        return;
    };

    let response = request(port, "/dav/read.bin");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("DAV read-fault response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "DAV read failure rewrote the response:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "DAV read failure did not truncate the response"
    );
    server.assert_running();
}

fn cache_read_eio_falls_back_to_upstream() {
    let port = reserve_port();
    let body = "c".repeat(128 * 1024);
    let Some(upstream) = repeating_upstream(&body, 2) else {
        return;
    };
    let directory = TestDirectory::new("read-cache");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &cache_read_config(port, upstream.port),
        FaultSpec {
            path: "/cache/",
            error: EIO,
            operations: "read",
        },
    ) else {
        return;
    };

    let first = request(port, "/cache");
    assert!(
        first.starts_with("HTTP/1.1 200 OK\r\n"),
        "cache fill failed before the read fault:\n{first}"
    );
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "first cache response was not a miss:\n{first}"
    );
    wait_for_files(&server.temp_path("cache"), "cache fill");

    let second = request(port, "/cache");
    assert!(
        second.starts_with("HTTP/1.1 200 OK\r\n"),
        "cache read failure did not fall back to upstream:\n{second}"
    );
    assert!(
        second.contains(&body),
        "cache read failure returned a truncated upstream body"
    );
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: miss"),
        "cache read failure was treated as a hit:\n{second}"
    );
    server.assert_running();
    upstream
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    upstream
        .requests
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
}

fn request_body_read_eio_maps_to_500() {
    let port = reserve_port();
    let Some(upstream) = fake_upstream("request-body-read") else {
        return;
    };
    let directory = TestDirectory::new("read-request-body");
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &request_body_upload_config(port, upstream.port),
        FaultSpec {
            path: "/body-temp/",
            error: EIO,
            operations: "read",
        },
    ) else {
        return;
    };

    let response = post_large(port, "/upload", 128 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 500 "),
        "request body replay read failure should map to 500:\n{response}"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("body-temp"), "request body read");
}

fn static_config(port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log stderr notice;

            events {{ worker_connections 128; }}

            http {{
                default_type text/plain;
                access_log off;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;
                }}
            }}
        "#
    )
}

fn cache_read_config(port: u16, upstream_port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log stderr notice;

            events {{ worker_connections 128; }}

            http {{
                default_type text/plain;
                access_log off;
                proxy_cache_path cache keys_zone=read_fault_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    add_header X-Cache-Status $upstream_cache_status always;

                    location /cache {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_cache read_fault_cache;
                        proxy_cache_valid 200 1m;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn request_body_upload_config(port: u16, upstream_port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log stderr notice;

            events {{ worker_connections 128; }}

            http {{
                default_type text/plain;
                access_log off;
                client_body_buffer_size 4k;
                client_body_temp_path body-temp;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /upload {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

struct RepeatingUpstream {
    port: u16,
    requests: mpsc::Receiver<()>,
}

fn repeating_upstream(body: &str, count: usize) -> Option<RepeatingUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime cache-read test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind repeating fake upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let body = body.to_owned();
    let (sender, requests) = mpsc::channel();
    thread::spawn(move || {
        for _ in 0..count {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            read_request_head(&mut stream)?;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/plain\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\
                 \r\n{}",
                body.len(),
                body
            )?;
            stream.flush()?;
            sender.send(()).ok();
        }
        Ok::<(), io::Error>(())
    });
    Some(RepeatingUpstream { port, requests })
}

fn wait_for_files(root: &Path, label: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if !files_under(root).is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{label} did not create a file under {}",
            root.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return files;
    };
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

fn post_large(port: u16, target: &str, length: usize) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect request body client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "POST {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {length}\r\n\
         Connection: close\r\n\r\n"
    )
    .unwrap();
    if let Err(error) = stream.write_all(&vec![b'z'; length]) {
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
            ),
            "write request body: {error}"
        );
    }
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}
