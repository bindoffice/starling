use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{TestDirectory, request};

const PRELOAD_SOURCE: &str = include_str!("disk_faults.c");

pub(super) const ENOSPC: i32 = libc::ENOSPC;
pub(super) const EDQUOT: i32 = libc::EDQUOT;
pub(super) const EIO: i32 = libc::EIO;
pub(super) const EMFILE: i32 = libc::EMFILE;
pub(super) const ENFILE: i32 = libc::ENFILE;
pub(super) const EROFS: i32 = libc::EROFS;

pub(super) struct FaultSpec<'a> {
    pub path: &'a str,
    pub error: i32,
    pub operations: &'a str,
}

pub(super) struct FaultServer {
    directory: TestDirectory,
    child: Child,
}

impl FaultServer {
    pub(super) fn start(
        directory: TestDirectory,
        port: u16,
        config: &str,
        fault: FaultSpec<'_>,
    ) -> Option<Self> {
        Self::start_with_rules(directory, port, config, std::slice::from_ref(&fault), false)
    }

    pub(super) fn start_with_rules(
        directory: TestDirectory,
        port: u16,
        config: &str,
        faults: &[FaultSpec<'_>],
        use_rules: bool,
    ) -> Option<Self> {
        if compiler_available().is_none() {
            eprintln!("skipping disk fault migration: cc is not available");
            return None;
        }
        directory.write("conf/nginx.conf", config);
        directory.write("disk-faults.c", PRELOAD_SOURCE);
        let preload = directory.path().join("disk-faults.so");
        let status = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&preload)
            .arg(directory.path().join("disk-faults.c"))
            .arg("-ldl")
            .status()
            .expect("compile disk fault preload");
        assert!(status.success(), "failed to compile disk fault preload");

        let stdout = fs::File::create(directory.path().join("logs/stdout.log")).unwrap();
        let stderr = fs::File::create(directory.path().join("logs/stderr.log")).unwrap();
        let legacy = faults
            .first()
            .expect("at least one disk fault rule is required");
        let rules = faults
            .iter()
            .map(|fault| {
                assert!(
                    !fault.path.contains(['|', ';']),
                    "fault paths cannot contain rule delimiters: {}",
                    fault.path
                );
                assert!(
                    !fault.operations.contains(['|', ';']),
                    "fault operations cannot contain rule delimiters: {}",
                    fault.operations
                );
                format!("{}|{}|{}", fault.path, fault.error, fault.operations)
            })
            .collect::<Vec<_>>()
            .join(";");
        let mut command = Command::new(env!("CARGO_BIN_EXE_starling"));
        command
            .arg("-p")
            .arg(directory.path())
            .arg("-c")
            .arg("conf/nginx.conf")
            .env("LD_PRELOAD", preload)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        if use_rules {
            command.env("STARLING_FAULT_RULES", rules);
        } else {
            command
                .env("STARLING_FAULT_MATCH", legacy.path)
                .env("STARLING_FAULT_ERRNO", legacy.error.to_string())
                .env("STARLING_FAULT_OPS", legacy.operations);
        }
        let child = command
            .spawn()
            .expect("start starling with disk fault injection");
        let mut server = Self { directory, child };
        server.wait_until_ready(port);
        thread::sleep(Duration::from_millis(100));
        Some(server)
    }

    pub(super) fn temp_path(&self, relative: &str) -> PathBuf {
        self.directory.path().join(relative)
    }

    pub(super) fn write_config(&self, contents: &str) {
        self.directory.write("conf/nginx.conf", contents);
    }

    pub(super) fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "starling exited after a recoverable disk fault:\n{}",
            self.diagnostics()
        );
    }

    pub(super) fn reload(&mut self) {
        self.assert_running();
        let result = unsafe { libc::kill(self.child.id() as i32, libc::SIGHUP) };
        assert_eq!(
            result,
            0,
            "send SIGHUP failed: {}\n{}",
            std::io::Error::last_os_error(),
            self.diagnostics()
        );
    }

    pub(super) fn shutdown_gracefully(&mut self) {
        self.assert_running();
        let result = unsafe { libc::kill(self.child.id() as i32, libc::SIGQUIT) };
        assert_eq!(
            result,
            0,
            "send SIGQUIT failed: {}\n{}",
            std::io::Error::last_os_error(),
            self.diagnostics()
        );
    }

    pub(super) fn wait_for_exit(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "starling exited with {status}:\n{}",
                    self.diagnostics()
                );
                return;
            }
            if Instant::now() >= deadline {
                panic!(
                    "starling did not exit within {timeout:?}:\n{}",
                    self.diagnostics()
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_until_ready(&mut self, port: u16) {
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!(
                    "starling exited during disk-fault startup with {status}:\n{}",
                    self.diagnostics()
                );
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for disk-fault starling listener:\n{}",
                self.diagnostics()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn diagnostics(&self) -> String {
        let stderr = fs::read_to_string(self.directory.path().join("logs/stderr.log"))
            .unwrap_or_else(|error| format!("<cannot read stderr: {error}>"));
        format!("stderr:\n{stderr}")
    }
}

impl Drop for FaultServer {
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

pub(super) fn compiler_available() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join("cc"))
        .find(|candidate| candidate.is_file())
}

pub(super) fn wait_for_no_files(root: &Path, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let files = files_under(root);
        if files.is_empty() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label} fault files were not cleaned up: {files:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn assert_complete_stream(port: u16, target: &str, body: &str, label: &str) {
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

pub(super) fn response_config(port: u16, upstream_port: u16) -> String {
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

pub(super) fn cache_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache keys_zone=disk_fault_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /cache {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_cache disk_fault_cache;
                        proxy_cache_valid 200 1m;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

pub(super) fn store_config(port: u16, upstream_port: u16) -> String {
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

pub(super) fn request_body_config(port: u16) -> String {
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

pub(super) fn dav_config(port: u16) -> String {
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

                    location /dav/ {{
                        dav_methods PUT;
                        create_full_put_path on;
                        dav_access user:rw group:r all:r;
                    }}
                }}
            }}
        "#
    )
}
