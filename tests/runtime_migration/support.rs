use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod fake_upstream;
mod tls_client;
pub(crate) use fake_upstream::{
    fake_upstream, fake_upstream_with_delay, fake_upstream_with_status, read_request_head,
};
pub(crate) use tls_client::tls_request_unverified;

static COUNTER: AtomicU64 = AtomicU64::new(0);
static NEXT_TEST_PORT: AtomicU32 = AtomicU32::new(0);
const TEST_PORT_BASE: u16 = 20_000;
const TEST_PORT_SPAN: u16 = 8_000;

pub(crate) struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    pub(crate) fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "starling-runtime-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("conf")).unwrap();
        fs::create_dir_all(path.join("html/assets")).unwrap();
        fs::create_dir_all(path.join("logs")).unwrap();
        Self { path }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn write(&self, relative: &str, contents: &str) {
        let path = self.path.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub(crate) struct RunningServer {
    directory: TestDirectory,
    child: Child,
    port: u16,
}

impl RunningServer {
    pub(crate) fn start(directory: TestDirectory, port: u16, config: &str) -> Self {
        directory.write("conf/nginx.conf", config);
        let stdout = fs::File::create(directory.path().join("logs/stdout.log")).unwrap();
        let stderr = fs::File::create(directory.path().join("logs/stderr.log")).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_starling"))
            .arg("-p")
            .arg(directory.path())
            .arg("-c")
            .arg("conf/nginx.conf")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("start starling binary");
        let mut server = Self {
            directory,
            child,
            port,
        };
        match server.wait_until_ready() {
            WaitResult::Ready => {
                // Listener tasks start just before the signal handlers are installed.
                thread::sleep(Duration::from_millis(100));
                server
            }
            WaitResult::Failed(error) => {
                server.stop();
                panic!("{error}");
            }
        }
    }

    pub(crate) fn get(&self, target: &str) -> String {
        request(self.port, target)
    }

    pub(crate) fn get_with_host(&self, target: &str, host: &str) -> String {
        request_with_host(self.port, target, host)
    }

    pub(crate) fn write_config(&self, contents: &str) {
        self.directory.write("conf/nginx.conf", contents);
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn directory_path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn reload(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "starling exited before reload:\n{}",
            self.diagnostics()
        );
        let result = rustix::process::kill_process(
            rustix::process::Pid::from_child(&self.child),
            rustix::process::Signal::HUP,
        );
        assert!(
            result.is_ok(),
            "send SIGHUP failed: {result:?}\n{}",
            self.diagnostics()
        );
    }

    pub(crate) fn assert_running(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "starling exited unexpectedly:\n{}",
            self.diagnostics()
        );
    }

    pub(crate) fn shutdown_gracefully(&mut self) {
        self.assert_running();
        let result = rustix::process::kill_process(
            rustix::process::Pid::from_child(&self.child),
            rustix::process::Signal::QUIT,
        );
        assert!(
            result.is_ok(),
            "send SIGQUIT failed: {result:?}\n{}",
            self.diagnostics()
        );
    }

    pub(crate) fn wait_for_exit(&mut self, timeout: Duration) {
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

    fn wait_until_ready(&mut self) -> WaitResult {
        let address = SocketAddr::from(([127, 0, 0, 1], self.port));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                let diagnostics = self.diagnostics();
                return WaitResult::Failed(format!(
                    "starling exited during startup with {status}:\n{diagnostics}"
                ));
            }
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                if let Some(status) = self.child.try_wait().unwrap() {
                    let diagnostics = self.diagnostics();
                    return WaitResult::Failed(format!(
                        "starling exited after accepting a startup probe with {status}:\n{diagnostics}"
                    ));
                }
                return WaitResult::Ready;
            }
            if Instant::now() >= deadline {
                return WaitResult::Failed(format!(
                    "timed out waiting for 127.0.0.1:{}:\n{}",
                    self.port,
                    self.diagnostics()
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn diagnostics(&self) -> String {
        let stderr = fs::read_to_string(self.directory.path().join("logs/stderr.log"))
            .unwrap_or_else(|error| format!("<cannot read stderr: {error}>"));
        let error_log = fs::read_to_string(self.directory.path().join("logs/error.log"))
            .unwrap_or_else(|error| format!("<cannot read error log: {error}>"));
        format!("stderr:\n{stderr}\nerror log:\n{error_log}")
    }

    fn stop(&mut self) {
        if self.child.try_wait().unwrap().is_some() {
            return;
        }
        let _ = rustix::process::kill_process(
            rustix::process::Pid::from_child(&self.child),
            rustix::process::Signal::TERM,
        );
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

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
    }
}

enum WaitResult {
    Ready,
    Failed(String),
}

pub(crate) fn reserve_port() -> u16 {
    // Keep server ports below Linux's default ephemeral range so bind(0) helpers cannot steal them.
    let process_offset = (std::process::id() % u32::from(TEST_PORT_SPAN)) as u16;
    for _ in 0..TEST_PORT_SPAN {
        let offset = NEXT_TEST_PORT.fetch_add(1, Ordering::Relaxed) as u16;
        let port = TEST_PORT_BASE + (process_offset + offset) % TEST_PORT_SPAN;
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                return port;
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => continue,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => panic!(
                "runtime migration tests require local TCP bind permission; run without network sandbox: {error}"
            ),
            Err(error) => panic!("reserve test port {port}: {error}"),
        }
    }
    panic!("unable to reserve a runtime migration test port");
}

pub(crate) fn request(port: u16, target: &str) -> String {
    request_with_host(port, target, "localhost")
}

pub(crate) fn request_with_host(port: u16, target: &str, host: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to starling");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    String::from_utf8_lossy(&response).into_owned()
}

pub(crate) fn assert_status(response: &str, status: u16) {
    let first_line = response.lines().next().unwrap_or_default();
    assert!(
        first_line.starts_with(&format!("HTTP/1.1 {status}")),
        "expected HTTP {status}, got:\n{response}"
    );
}

pub(crate) struct Http1Connection {
    stream: TcpStream,
    buffered: Vec<u8>,
}

impl Http1Connection {
    pub(crate) fn connect(port: u16) -> Self {
        let stream = TcpStream::connect(("127.0.0.1", port)).expect("connect keepalive client");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        Self {
            stream,
            buffered: Vec::new(),
        }
    }

    pub(crate) fn get(&mut self, target: &str) -> String {
        write!(
            self.stream,
            "GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n"
        )
        .unwrap();
        self.read_response()
    }

    pub(crate) fn into_stream(self) -> TcpStream {
        self.stream
    }

    fn read_response(&mut self) -> String {
        let (head_end, content_length) = loop {
            if let Some(head_end) = find_bytes(&self.buffered, b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&self.buffered[..head_end]);
                let content_length = head.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                break (head_end.saturating_add(4), content_length);
            }
            self.read_more();
        };
        let content_length = content_length.expect("test response must have Content-Length");
        let message_end = head_end + content_length;
        while self.buffered.len() < message_end {
            self.read_more();
        }
        let message = self.buffered.drain(..message_end).collect::<Vec<_>>();
        String::from_utf8_lossy(&message).into_owned()
    }

    fn read_more(&mut self) {
        let mut chunk = [0_u8; 4096];
        let count = self.stream.read(&mut chunk).expect("read HTTP response");
        assert!(
            count > 0,
            "keepalive connection closed before response completed"
        );
        self.buffered.extend_from_slice(&chunk[..count]);
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
