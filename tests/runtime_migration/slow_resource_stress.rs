use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use super::reload_stress_support::ProcessResources;
use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

const CLIENTS: usize = 8;
const RESET_CLIENTS: usize = 3;
const RELOADS: u64 = 16;
const BODY_SIZE: usize = 1024 * 1024;
const CHUNK_SIZE: usize = 32 * 1024;
const CHUNK_DELAY: Duration = Duration::from_millis(3);

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn slow_clients_resets_reloads_and_cache_store_keep_resources_bounded() {
    let port = reserve_port();
    let Some(upstream) = SlowUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("slow-resource-stress");
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));
    wait_for_version(&server, "rev-0");
    thread::sleep(Duration::from_millis(250));
    let baseline = ProcessResources::capture(server.pid());

    let barrier = Arc::new(Barrier::new(CLIENTS + 1));
    let mut clients = Vec::new();
    for index in 0..CLIENTS {
        let barrier = barrier.clone();
        clients.push(thread::spawn(move || {
            barrier.wait();
            request_slow(port, index, index < RESET_CLIENTS)
        }));
    }
    barrier.wait();
    upstream.wait_for_requests(CLIENTS, Duration::from_secs(10));

    let mut samples = Vec::new();
    for revision in 1..=RELOADS {
        server.write_config(&config(port, upstream.port, revision));
        server.reload();
        wait_for_version(&server, &format!("rev-{revision}"));
        thread::sleep(Duration::from_millis(5));
        if let Some(resources) = ProcessResources::capture(server.pid()) {
            eprintln!(
                "slow resource sample revision={revision} fds={} tasks={} rss_kib={}",
                resources.fds, resources.tasks, resources.rss_kib
            );
            samples.push(resources);
        }
    }

    for (index, client) in clients.into_iter().enumerate() {
        let response = client.join().expect("slow client thread panicked");
        if index >= RESET_CLIENTS {
            let response = response.expect("healthy slow client did not finish");
            assert_status(&String::from_utf8_lossy(&response), 200);
            let (_, body) = split_response(&response);
            assert_eq!(body.len(), BODY_SIZE, "slow client {index} body length");
            assert!(body.iter().all(|byte| *byte == b'x'));
        }
    }

    wait_for_stored_items(&server);
    let probe = server.get("/probe");
    assert_status(&probe, 200);
    assert!(
        probe.ends_with("probe"),
        "post-reload proxy failed:\n{probe}"
    );

    server.write_config(&config(port, upstream.port, RELOADS));
    server.reload();
    wait_for_version(&server, &format!("rev-{RELOADS}"));
    upstream.wait_for_no_active(Duration::from_secs(5));
    wait_until(Duration::from_secs(5), "temporary cleanup", || {
        files_under(&server.directory_path().join("temp")).is_empty()
    });

    thread::sleep(Duration::from_millis(250));
    let final_resources = ProcessResources::capture(server.pid());
    if let (Some(baseline), Some(final_resources)) = (baseline, final_resources) {
        let peak_rss_kib = samples
            .iter()
            .map(|sample| sample.rss_kib)
            .chain(std::iter::once(final_resources.rss_kib))
            .max()
            .unwrap_or(final_resources.rss_kib);
        let peak_tasks = samples
            .iter()
            .map(|sample| sample.tasks)
            .chain(std::iter::once(final_resources.tasks))
            .max()
            .unwrap_or(final_resources.tasks);
        assert!(
            final_resources.fds <= baseline.fds + 4,
            "FDs grew under mixed slow pressure: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_tasks <= baseline.tasks + 32,
            "task peak grew too far under mixed slow pressure: baseline={baseline:?} peak={peak_tasks}"
        );
        assert!(
            final_resources.tasks <= baseline.tasks + 32,
            "tasks did not settle after mixed slow pressure: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "RSS grew under mixed slow pressure: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_rss_kib <= baseline.rss_kib + 96 * 1024,
            "RSS peak grew too far: baseline={baseline:?} peak={peak_rss_kib} KiB"
        );
    }
    assert_eq!(
        upstream.stats.requests(),
        CLIENTS + 1,
        "unexpected upstream request count: active={}",
        upstream.stats.active()
    );
    server.assert_running();
    upstream.stop();
}

fn request_slow(port: u16, index: usize, reset: bool) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect slow client");
    socket2::SockRef::from(&stream)
        .set_recv_buffer_size(4 * 1024)
        .expect("set slow client receive buffer");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("set slow client read timeout");
    write!(
        stream,
        "GET /item/{index} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("write slow client request");

    if reset {
        let mut prefix = [0_u8; 4096];
        let read = stream.read(&mut prefix).expect("read reset client prefix");
        assert!(read > 0, "reset client received no response prefix");
        socket2::SockRef::from(&stream)
            .set_linger(Some(Duration::ZERO))
            .expect("set reset client linger");
        drop(stream);
        return None;
    }

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read complete slow response");
    Some(response)
}

fn wait_for_stored_items(server: &RunningServer) {
    for index in 0..CLIENTS {
        let path = server.directory_path().join(format!("html/item/{index}"));
        wait_until(Duration::from_secs(10), "cache/store publication", || {
            fs::metadata(&path).is_ok_and(|metadata| metadata.len() == BODY_SIZE as u64)
        });
    }
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

fn split_response(response: &[u8]) -> (&[u8], &[u8]) {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("slow response headers")
        + 4;
    response.split_at(head_end)
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
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

    fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
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
                eprintln!("skipping slow resource stress: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind slow resource upstream: {error}"),
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
        wait_until(timeout, "slow upstream requests", || {
            self.stats.requests() >= expected
        });
    }

    fn wait_for_no_active(&self, timeout: Duration) {
        wait_until(timeout, "slow upstream connection drain", || {
            self.stats.active() == 0
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
            Err(error) => panic!("slow upstream accept failed: {error}"),
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
    loop {
        let request = match read_request_head(&mut stream) {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                break;
            }
            Err(error) => panic!("slow upstream request failed: {error}"),
        };
        stats.requests.fetch_add(1, Ordering::Relaxed);
        if request.starts_with("GET /probe HTTP/1.1\r\n") {
            if write_probe(&mut stream).is_err() {
                break;
            }
        } else if write_slow_response(&mut stream).is_err() {
            break;
        }
    }
    stats.active.fetch_sub(1, Ordering::Relaxed);
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok(None);
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
        }
        if request.len() > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "slow upstream request head was too large",
            ));
        }
    }
}

fn write_probe(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: keep-alive\r\n\r\nprobe"
    )?;
    stream.flush()
}

fn write_slow_response(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {BODY_SIZE}\r\nConnection: keep-alive\r\n\r\n"
    )?;
    let chunk = vec![b'x'; CHUNK_SIZE];
    for _ in 0..BODY_SIZE / CHUNK_SIZE {
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
            worker_shutdown_timeout 250ms;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 512;
            }}

            http {{
                default_type text/plain;
                access_log off;
                keepalive_requests 100000000;
                client_body_temp_path temp/client-body;
                proxy_temp_path temp/proxy;
                proxy_cache_path cache levels=1:2 keys_zone=slow_cache:1m inactive=60m;

                upstream slow_backend {{
                    server 127.0.0.1:{upstream_port};
                    keepalive 4;
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location / {{
                        proxy_pass http://slow_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                        proxy_buffer_size 1k;
                        proxy_buffers 4 4k;
                        proxy_busy_buffers_size 8k;
                        proxy_temp_file_write_size 4k;
                        proxy_max_temp_file_size 8m;
                        proxy_read_timeout 5s;
                        proxy_ignore_client_abort on;
                        proxy_cache slow_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}
