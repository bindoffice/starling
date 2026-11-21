use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{TestDirectory, fake_upstream, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn file_size_limit_truncates_committed_response_and_cleans_spill_file() {
    let port = reserve_port();
    let body = "x".repeat(64 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("file-size-limit");
    let config = config(port, upstream.port);
    let mut server = LimitedServer::start(directory, port, &config);

    let response = request(port, "/spill");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("limited response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "committed response was rewritten after EFBIG:\n{response}"
    );
    assert!(
        response_body.len() < body.len(),
        "file-size limit did not truncate the spooled response"
    );
    assert!(
        !response_body.is_empty(),
        "the buffered prefix should reach the downstream"
    );
    upstream.captured();
    server.assert_running();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let temporary = files_under(&server.directory.path().join("proxy-temp"));
        if temporary.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failed spill files were not cleaned up: {temporary:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn file_size_limit_keeps_downstream_complete_when_cache_and_store_fail() {
    let port = reserve_port();
    let body = "y".repeat(64 * 1024);
    let Some(upstream) = fake_upstream(&body) else {
        return;
    };
    let directory = TestDirectory::new("file-size-cache-store");
    let config = cache_store_config(port, upstream.port);
    let mut server = LimitedServer::start(directory, port, &config);

    let response = request(port, "/cache-store");
    let (head, response_body) = response
        .split_once("\r\n\r\n")
        .expect("cache/store response headers");
    assert!(
        head.starts_with("HTTP/1.1 200 OK\r\n"),
        "cache/store write failure rewrote the downstream response:\n{head}"
    );
    assert_eq!(
        response_body.len(),
        body.len(),
        "cache/store write failure truncated the downstream body"
    );
    assert_eq!(response_body, body);
    upstream.captured();
    server.assert_running();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let cache_files = files_under(&server.directory.path().join("cache"));
        let store_files = files_under(&server.directory.path().join("html"));
        let published_store = server.directory.path().join("html/cache-store");
        if cache_files.is_empty() && store_files.is_empty() && !published_store.exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failed cache/store files were not cleaned up: cache={cache_files:?}, \
             html={store_files:?}, store_exists={}",
            published_store.exists()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn file_size_limit_maps_request_body_spool_failure_and_cleans_temp_file() {
    let port = reserve_port();
    let directory = TestDirectory::new("file-size-request-body");
    let config = request_body_config(port);
    let mut server = LimitedServer::start(directory, port, &config);

    let response = post_large(port, "/upload", 64 * 1024);
    assert!(
        response.starts_with("HTTP/1.1 500 "),
        "request body spool EFBIG should map to 500:\n{response}"
    );
    server.assert_running();

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let temporary = files_under(&server.directory.path().join("body-temp"));
        if temporary.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "failed request body files were not cleaned up: {temporary:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

struct LimitedServer {
    directory: TestDirectory,
    child: Child,
}

impl LimitedServer {
    fn start(directory: TestDirectory, port: u16, config: &str) -> Self {
        directory.write("conf/nginx.conf", config);
        let stdout = fs::File::create(directory.path().join("logs/stdout.log")).unwrap();
        let stderr = fs::File::create(directory.path().join("logs/stderr.log")).unwrap();
        let binary = env!("CARGO_BIN_EXE_starling");
        let child = Command::new("sh")
            .arg("-c")
            .arg("trap '' XFSZ; ulimit -f 8; exec \"$1\" -p \"$2\" -c conf/nginx.conf")
            .arg("starling-file-limit")
            .arg(binary)
            .arg(directory.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("start starling binary with a file-size limit");
        let mut server = Self { directory, child };
        server.wait_until_ready(port);
        thread::sleep(Duration::from_millis(100));
        server
    }

    fn wait_until_ready(&mut self, port: u16) {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "starling exited during startup with {status}:\n{}",
                    self.diagnostics()
                );
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for limited starling listener:\n{}",
                self.diagnostics()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "starling exited after a recoverable EFBIG:\n{}",
            self.diagnostics()
        );
    }

    fn diagnostics(&self) -> String {
        let stderr = fs::read_to_string(self.directory.path().join("logs/stderr.log"))
            .unwrap_or_else(|error| format!("<cannot read stderr: {error}>"));
        let error_log = fs::read_to_string(self.directory.path().join("logs/error.log"))
            .unwrap_or_else(|error| format!("<cannot read error log: {error}>"));
        format!("stderr:\n{stderr}\nerror log:\n{error_log}")
    }
}

impl Drop for LimitedServer {
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

fn files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn config(port: u16, upstream_port: u16) -> String {
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

                    location /spill {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffers 2 4k;
                        proxy_busy_buffers_size 4k;
                        proxy_temp_file_write_size 4k;
                        proxy_temp_path proxy-temp;
                        proxy_max_temp_file_size 64k;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

fn cache_store_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache keys_zone=limit_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location /cache-store {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_cache limit_cache;
                        proxy_cache_valid 200 1m;
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
            error_log logs/error.log notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log logs/access.log;
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
    let body = vec![b'z'; length];
    if let Err(error) = stream.write_all(&body) {
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
