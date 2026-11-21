use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::disk_fault_support::{
    EIO, FaultServer, FaultSpec, compiler_available, wait_for_no_files,
};
use super::support::{TestDirectory, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn one_request_combines_body_response_and_store_faults() {
    if compiler_available().is_none() {
        eprintln!("skipping disk multistage migration: cc is not available");
        return;
    }

    let port = reserve_port();
    let request_body = "b".repeat(64 * 1024);
    let response_body = "r".repeat(128 * 1024);
    let Some((upstream_port, captured)) = body_response_upstream(&response_body) else {
        return;
    };
    let config = multistage_config(port, upstream_port);
    let directory = TestDirectory::new("disk-multistage-request");
    let rules = [
        FaultSpec {
            path: "/state/body/",
            error: EIO,
            operations: "close",
        },
        FaultSpec {
            path: "/state/proxy/",
            error: EIO,
            operations: "write,unlink,close",
        },
        FaultSpec {
            path: "/state/html/",
            error: EIO,
            operations: "write,unlink,close",
        },
    ];
    let Some(mut server) = FaultServer::start_with_rules(directory, port, &config, &rules, true)
    else {
        return;
    };

    let response = post(port, "/combined", &request_body);
    let (head, received_body) = response
        .split_once("\r\n\r\n")
        .expect("multistage response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "the response spill failure rewrote the committed status:\n{head}"
    );
    assert!(
        !received_body.is_empty() && received_body.len() < response_body.len(),
        "the response spill failure should truncate only after a committed prefix"
    );

    let upstream_request = captured
        .recv_timeout(Duration::from_secs(5))
        .expect("multistage upstream capture timed out");
    assert!(
        upstream_request.ends_with(request_body.as_bytes()),
        "the request body close failure truncated the upstream request"
    );

    server.assert_running();
    let health = request(port, "/health");
    assert!(
        health.starts_with("HTTP/1.1 404 "),
        "worker did not serve a request after the multistage failures:\n{health}"
    );
    wait_for_no_files(&server.temp_path("state/body"), "request body close");
    assert!(
        !server.temp_path("state/html/combined").exists(),
        "store published its target after write and cleanup failures"
    );
    let residue = files_under(&server.temp_path("state"));
    assert!(
        residue.len() >= 2,
        "response spill and store cleanup residue should remain observable: {residue:?}"
    );
    assert!(
        residue.iter().all(|path| is_temporary_artifact(path)),
        "a multistage failure published a non-temporary artifact: {residue:?}"
    );
}

fn multistage_config(port: u16, upstream_port: u16) -> String {
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
                client_body_temp_path state/body;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root state/html;

                    location /combined {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_store on;
                        proxy_buffers 2 4k;
                        proxy_busy_buffers_size 4k;
                        proxy_temp_file_write_size 4k;
                        proxy_temp_path state/proxy;
                        proxy_max_temp_file_size 512k;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn body_response_upstream(response_body: &str) -> Option<(u16, Receiver<Vec<u8>>)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    let port = listener.local_addr().unwrap().port();
    let response_body = response_body.to_owned();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept multistage upstream");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let request = read_request(&mut stream);
        sender.send(request).unwrap();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        )
        .unwrap();
    });
    Some((port, receiver))
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let count = stream.read(&mut buffer).expect("read upstream request");
        assert!(count > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..count]);
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
        let count = stream
            .read(&mut buffer)
            .expect("read upstream request body");
        assert!(count > 0, "upstream closed before request body");
        request.extend_from_slice(&buffer[..count]);
    }
    request
}

fn post(port: u16, target: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect multistage client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "POST {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    String::from_utf8_lossy(&response).into_owned()
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

fn is_temporary_artifact(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with(".starling-tmp-")
                || name.starts_with(".starling-store-")
                || name.starts_with(".starling-cache-")
        })
}
