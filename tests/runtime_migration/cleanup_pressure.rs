use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{RunningServer, TestDirectory, read_request_head, reserve_port};

const CLIENTS: usize = 6;
const RESET_CLIENTS: usize = 2;
const BODY_SIZE: usize = 1024 * 1024;
const PREFIX_SIZE: usize = 128 * 1024;
const CHUNK_SIZE: usize = 16 * 1024;
const UPSTREAM_PAUSE: Duration = Duration::from_millis(750);
const CHUNK_DELAY: Duration = Duration::from_millis(5);

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn reload_and_sigquit_clean_inflight_cache_store_temps_after_resets() {
    let port = reserve_port();
    let Some(upstream) = SlowUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("cleanup-pressure");
    let root = directory.path().to_path_buf();
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));
    wait_for_version(&server, "rev-0");

    let barrier = Arc::new(Barrier::new(CLIENTS + 1));
    let mut clients = Vec::new();
    for index in 0..CLIENTS {
        let barrier = barrier.clone();
        clients.push(thread::spawn(move || {
            barrier.wait();
            request_slow(port, index, index < RESET_CLIENTS);
        }));
    }
    barrier.wait();
    upstream.wait_for_requests(CLIENTS, Duration::from_secs(10));
    wait_until(
        Duration::from_secs(10),
        "cache/store temporary files",
        || cache_store_temps(&root, 0).len() >= CLIENTS * 2,
    );

    server.write_config(&config(port, upstream.port, 1));
    server.reload();
    wait_for_version(&server, "rev-1");

    server.shutdown_gracefully();
    for client in clients {
        client.join().expect("cleanup pressure client panicked");
    }
    server.wait_for_exit(Duration::from_secs(5));
    upstream.wait_for_no_active(Duration::from_secs(5));

    for revision in [0, 1] {
        wait_until(Duration::from_secs(5), "cache/store temp cleanup", || {
            cache_store_temps(&root, revision).is_empty()
        });
    }
    assert!(
        response_spills(&root, 0).is_empty(),
        "response spill remained after shutdown: {:?}",
        response_spills(&root, 0)
    );
    assert!(
        response_spills(&root, 1).is_empty(),
        "new-generation response spill was polluted: {:?}",
        response_spills(&root, 1)
    );
    upstream.stop();
}

fn request_slow(port: u16, index: usize, reset: bool) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect cleanup client");
    socket2::SockRef::from(&stream)
        .set_recv_buffer_size(4 * 1024)
        .expect("set cleanup client receive buffer");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set cleanup client read timeout");
    write!(
        stream,
        "GET /item/{index} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("write cleanup client request");

    if reset {
        let mut prefix = [0_u8; 4096];
        let read = stream.read(&mut prefix).expect("read reset client prefix");
        assert!(read > 0, "reset client received no response prefix");
        socket2::SockRef::from(&stream)
            .set_linger(Some(Duration::ZERO))
            .expect("set cleanup client linger");
        drop(stream);
        return;
    }

    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
}

fn wait_for_version(server: &RunningServer, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = server.get("/version");
        if response.ends_with(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "reload did not publish {expected}:\n{response}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn cache_store_temps(root: &Path, revision: u64) -> Vec<PathBuf> {
    let base = root.join(format!("state-{revision}"));
    files_under(&base)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(".starling-cache-") || name.starts_with(".starling-store-")
                })
        })
        .collect()
}

fn response_spills(root: &Path, revision: u64) -> Vec<PathBuf> {
    files_under(&root.join(format!("temp-{revision}/proxy")))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".starling-"))
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

#[derive(Clone)]
struct SlowStats {
    active: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
}

impl SlowStats {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(AtomicUsize::new(0)),
        }
    }
}

struct SlowUpstream {
    port: u16,
    stats: SlowStats,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl SlowUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping cleanup pressure test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind cleanup pressure upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stats = SlowStats::new();
        let stop = Arc::new(AtomicBool::new(false));
        let accept = thread::spawn({
            let stats = stats.clone();
            let stop = stop.clone();
            move || accept_loop(listener, stats, stop)
        });
        Some(Self {
            port,
            stats,
            stop,
            accept: Some(accept),
        })
    }

    fn wait_for_requests(&self, expected: usize, timeout: Duration) {
        wait_until(timeout, "cleanup upstream requests", || {
            self.stats.requests.load(Ordering::Relaxed) >= expected
        });
    }

    fn wait_for_no_active(&self, timeout: Duration) {
        wait_until(timeout, "cleanup upstream connection drain", || {
            self.stats.active.load(Ordering::Relaxed) == 0
        });
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(listener: TcpListener, stats: SlowStats, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let stats = stats.clone();
                thread::spawn(move || serve_connection(stream, stats));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("cleanup pressure accept failed: {error}"),
        }
    }
}

fn serve_connection(mut stream: TcpStream, stats: SlowStats) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stats.active.fetch_add(1, Ordering::Relaxed);
    if read_request_head(&mut stream).is_ok() {
        stats.requests.fetch_add(1, Ordering::Relaxed);
        let _ = write_slow_response(&mut stream);
    }
    stats.active.fetch_sub(1, Ordering::Relaxed);
}

fn write_slow_response(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {BODY_SIZE}\r\nConnection: close\r\n\r\n"
    )?;
    stream.write_all(&vec![b'x'; PREFIX_SIZE])?;
    stream.flush()?;
    thread::sleep(UPSTREAM_PAUSE);
    let chunk = vec![b'x'; CHUNK_SIZE];
    for _ in 0..((BODY_SIZE - PREFIX_SIZE) / CHUNK_SIZE) {
        stream.write_all(&chunk)?;
        thread::sleep(CHUNK_DELAY);
    }
    stream.flush()
}

fn config(port: u16, upstream_port: u16, revision: u64) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            worker_shutdown_timeout 100ms;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 256;
            }}

            http {{
                default_type application/octet-stream;
                access_log off;
                client_body_temp_path temp-{revision}/client-body;
                proxy_temp_path temp-{revision}/proxy;
                proxy_cache_path state-{revision}/cache levels=1:2
                                 keys_zone=cleanup_cache_{revision}:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root state-{revision}/html;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location /item/ {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_buffer_size 1k;
                        proxy_buffers 4 1k;
                        proxy_busy_buffers_size 2k;
                        proxy_temp_file_write_size 1k;
                        proxy_max_temp_file_size 4m;
                        proxy_buffering off;
                        proxy_ignore_client_abort on;
                        proxy_cache cleanup_cache_{revision};
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}
