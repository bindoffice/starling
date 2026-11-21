use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{TestDirectory, fake_upstream, request, reserve_port};

const PRELOAD_SOURCE: &str = include_str!("enospc_preload.c");

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn enospc_faults_preserve_stream_semantics_and_clean_temp_files() {
    if command_available("cc").is_none() {
        eprintln!("skipping ENOSPC migration: cc is not available");
        return;
    }

    response_spill_enospc_truncates_after_commit();
    cache_enospc_keeps_downstream_complete_without_publishing();
    store_enospc_keeps_downstream_complete_without_publishing();
    request_body_enospc_maps_to_500_and_cleans_spool();
}

fn response_spill_enospc_truncates_after_commit() {
    let port = reserve_port();
    let body = "x".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("enospc-response");
    let mut server = EnospcServer::start(
        directory,
        port,
        &response_config(port, upstream.port),
        "/proxy-temp/",
    );

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("ENOSPC response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "committed response was rewritten after ENOSPC:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "ENOSPC did not truncate the spilled response"
    );
    assert!(
        !response_body.is_empty(),
        "the committed prefix should reach the downstream"
    );
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("proxy-temp"), "response spill");
}

fn cache_enospc_keeps_downstream_complete_without_publishing() {
    let port = reserve_port();
    let body = "c".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("enospc-cache");
    let mut server = EnospcServer::start(
        directory,
        port,
        &cache_config(port, upstream.port),
        "/cache/",
    );

    assert_complete_stream(port, "/cache", &body, "cache ENOSPC");
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("cache"), "cache");
}

fn store_enospc_keeps_downstream_complete_without_publishing() {
    let port = reserve_port();
    let body = "s".repeat(128 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("enospc-store");
    let mut server = EnospcServer::start(
        directory,
        port,
        &store_config(port, upstream.port),
        "/html/",
    );

    assert_complete_stream(port, "/stored", &body, "store ENOSPC");
    upstream.captured();
    server.assert_running();
    wait_for_no_files(&server.temp_path("html"), "store");
    assert!(
        !server.temp_path("html/stored").exists(),
        "store published a target after ENOSPC"
    );
}

fn request_body_enospc_maps_to_500_and_cleans_spool() {
    let port = reserve_port();
    let directory = TestDirectory::new("enospc-request-body");
    let mut server =
        EnospcServer::start(directory, port, &request_body_config(port), "/body-temp/");

    let response = post_large(port, "/upload", 128 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 500 "),
        "request body spool ENOSPC should map to 500:\n{response}"
    );
    server.assert_running();
    wait_for_no_files(&server.temp_path("body-temp"), "request body spool");
}

fn assert_complete_stream(port: u16, target: &str, body: &str, label: &str) {
    let response = request(port, target);
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("streaming response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "{label} rewrote the downstream response:\n{head}"
    );
    assert_eq!(
        response_body.len(),
        body.len(),
        "{label} truncated the downstream body"
    );
    assert_eq!(response_body, body, "{label} changed the downstream body");
}

fn wait_for_no_files(root: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let files = files_under(root);
        if files.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label} ENOSPC files were not cleaned up: {files:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

struct EnospcServer {
    directory: TestDirectory,
    child: Child,
}

impl EnospcServer {
    fn start(directory: TestDirectory, port: u16, config: &str, match_path: &str) -> Self {
        directory.write("conf/nginx.conf", config);
        directory.write("enospc-preload.c", PRELOAD_SOURCE);
        let preload = directory.path().join("enospc-preload.so");
        let status = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&preload)
            .arg(directory.path().join("enospc-preload.c"))
            .arg("-ldl")
            .status()
            .expect("compile ENOSPC preload");
        assert!(status.success(), "failed to compile ENOSPC preload");

        let stdout = fs::File::create(directory.path().join("logs/stdout.log")).unwrap();
        let stderr = fs::File::create(directory.path().join("logs/stderr.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_starling"))
            .arg("-p")
            .arg(directory.path())
            .arg("-c")
            .arg("conf/nginx.conf")
            .env("LD_PRELOAD", preload)
            .env("STARLING_ENOSPC_MATCH", match_path)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("start starling with ENOSPC injection");
        let mut server = Self { directory, child };
        server.wait_until_ready(port);
        thread::sleep(Duration::from_millis(100));
        server
    }

    fn temp_path(&self, relative: &str) -> PathBuf {
        self.directory.path().join(relative)
    }

    fn wait_until_ready(&mut self, port: u16) {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "starling exited during ENOSPC startup with {status}:\n{}",
                    self.diagnostics()
                );
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for ENOSPC starling listener:\n{}",
                self.diagnostics()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "starling exited after a recoverable ENOSPC:\n{}",
            self.diagnostics()
        );
    }

    fn diagnostics(&self) -> String {
        let stderr = fs::read_to_string(self.directory.path().join("logs/stderr.log"))
            .unwrap_or_else(|error| format!("<cannot read stderr: {error}>"));
        format!("stderr:\n{stderr}")
    }
}

impl Drop for EnospcServer {
    fn drop(&mut self) {
        if self.child.try_wait().unwrap().is_some() {
            return;
        }
        unsafe {
            libc::kill(self.child.id() as i32, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn command_available(command: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(command))
        .find(|candidate| candidate.is_file())
}

fn files_under(root: &Path) -> Vec<PathBuf> {
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

fn response_config(port: u16, upstream_port: u16) -> String {
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

                    location /spill {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffers 2 4k;
                        proxy_busy_buffers_size 4k;
                        proxy_temp_file_write_size 4k;
                        proxy_temp_path proxy-temp;
                        proxy_max_temp_file_size 512k;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn cache_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache keys_zone=enospc_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /cache {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_cache enospc_cache;
                        proxy_cache_valid 200 1m;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn store_config(port: u16, upstream_port: u16) -> String {
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

                    location /stored {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_store on;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn request_body_config(port: u16) -> String {
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
                        return 204;
                    }}
                }}
            }}
        "#
    )
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
