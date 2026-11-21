use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{Http1Connection, RunningServer, TestDirectory, assert_status, reserve_port};

const RELOADS: u64 = 24;
const TRAFFIC_CLIENTS: usize = 8;
const IDLE_CLIENTS: usize = 8;

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn repeated_reload_keepalive_traffic_and_sigquit_leave_no_runtime_leaks() {
    let port = reserve_port();
    let Some(upstream) = StressUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("reload-stress");
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));

    let mut control = Http1Connection::connect(port);
    wait_for_version(&mut control, "rev-0");
    let baseline = ProcessResources::capture(server.pid());

    let stop = Arc::new(AtomicBool::new(false));
    let mut traffic = Vec::new();
    for _ in 0..TRAFFIC_CLIENTS {
        let stop = stop.clone();
        traffic.push(thread::spawn(move || {
            let mut connection = Http1Connection::connect(port);
            let mut completed = 0_usize;
            while !stop.load(Ordering::Relaxed) {
                let response = connection.get("/proxied");
                assert_status(&response, 200);
                assert!(
                    response.contains("proxied"),
                    "unexpected proxied response:\n{response}"
                );
                completed += 1;
            }
            completed
        }));
    }

    for revision in 1..=RELOADS {
        server.write_config(&config(port, upstream.port, revision));
        server.reload();
        let version = format!("rev-{revision}");
        wait_for_version(&mut control, &version);

        // A proxied request after each reload forces a new generation to connect.
        let response = control.get("/proxied");
        assert_status(&response, 200);
        assert!(
            response.contains("proxied"),
            "proxied request after {version} failed:\n{response}"
        );
    }

    stop.store(true, Ordering::Relaxed);
    let completed = traffic
        .into_iter()
        .map(|client| client.join().expect("keepalive traffic client panicked"))
        .sum::<usize>();
    assert!(
        completed >= TRAFFIC_CLIENTS,
        "keepalive traffic did not run during reloads: {completed} requests"
    );

    // No request races this final clear, so every pooled upstream connection must close.
    server.write_config(&config(port, upstream.port, RELOADS));
    server.reload();
    wait_for_version(&mut control, &format!("rev-{RELOADS}"));
    upstream.wait_for_no_active(Duration::from_secs(5));

    assert!(
        upstream.stats.opened() >= RELOADS as usize,
        "each reload generation should establish an upstream connection: {:?}",
        upstream.stats.summary()
    );
    assert_eq!(
        upstream.stats.mixed_revisions(),
        Vec::<String>::new(),
        "an upstream keepalive connection served more than one config generation"
    );
    assert!(
        upstream.stats.revisions().len() >= 8,
        "reload stress did not exercise distinct proxy configurations: {:?}",
        upstream.stats.summary()
    );

    thread::sleep(Duration::from_millis(200));
    let final_resources = ProcessResources::capture(server.pid());
    if let (Some(baseline), Some(final_resources)) = (baseline, final_resources) {
        assert!(
            final_resources.fds <= baseline.fds + 4,
            "file descriptors grew across reloads: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "RSS grew across reloads: baseline={baseline:?} final={final_resources:?}"
        );
    }
    assert!(
        files_under(&server.directory_path().join("temp")).is_empty(),
        "reload stress left temporary files behind"
    );

    let mut idle = Vec::new();
    for _ in 0..IDLE_CLIENTS {
        let mut connection = Http1Connection::connect(port);
        wait_for_version(&mut connection, &format!("rev-{RELOADS}"));
        idle.push(connection.into_stream());
    }
    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    for mut connection in idle {
        wait_for_eof(&mut connection);
    }
    wait_for_eof(&mut control.into_stream());
    upstream.stop();
}

fn wait_for_version(connection: &mut Http1Connection, expected: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = connection.get("/version");
        if response.contains(expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "reload did not publish {expected}:\n{response}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_eof(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) =>
            {
                return;
            }
            Err(error) => panic!("idle keepalive client was not closed: {error}"),
        }
    }
}

#[derive(Debug)]
struct ProcessResources {
    fds: usize,
    rss_kib: u64,
}

impl ProcessResources {
    fn capture(pid: u32) -> Option<Self> {
        let fds = fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count();
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let rss_kib = status.lines().find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })?;
        Some(Self { fds, rss_kib })
    }
}

#[derive(Clone)]
struct UpstreamStats {
    opened: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    revisions: Arc<Mutex<HashSet<u64>>>,
    mixed: Arc<Mutex<Vec<String>>>,
}

impl UpstreamStats {
    fn new() -> Self {
        Self {
            opened: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            closed: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(AtomicUsize::new(0)),
            revisions: Arc::new(Mutex::new(HashSet::new())),
            mixed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn opened(&self) -> usize {
        self.opened.load(Ordering::Relaxed)
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    fn revisions(&self) -> HashSet<u64> {
        self.revisions.lock().unwrap().clone()
    }

    fn mixed_revisions(&self) -> Vec<String> {
        self.mixed.lock().unwrap().clone()
    }

    fn summary(&self) -> String {
        format!(
            "opened={} active={} closed={} requests={} revisions={:?}",
            self.opened(),
            self.active(),
            self.closed.load(Ordering::Relaxed),
            self.requests.load(Ordering::Relaxed),
            self.revisions()
        )
    }
}

struct StressUpstream {
    port: u16,
    stats: UpstreamStats,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl StressUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping runtime reload stress test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind stress upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stats = UpstreamStats::new();
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

    fn wait_for_no_active(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.stats.active() != 0 {
            assert!(
                Instant::now() < deadline,
                "reload did not close pooled upstream connections: {:?}",
                self.stats.summary()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(listener: TcpListener, stats: UpstreamStats, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let stats = stats.clone();
                thread::spawn(move || serve_connection(stream, stats));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("stress upstream accept failed: {error}"),
        }
    }
}

fn serve_connection(mut stream: TcpStream, stats: UpstreamStats) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let connection = stats.opened.fetch_add(1, Ordering::Relaxed) + 1;
    stats.active.fetch_add(1, Ordering::Relaxed);
    let mut revision = None;

    loop {
        let request = match read_request_head_or_end(&mut stream) {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(error) => panic!("stress upstream request failed: {error}"),
        };
        let current = header_value(&request, "x-config-revision")
            .and_then(|value| value.parse::<u64>().ok())
            .expect("proxied request must include X-Config-Revision");
        if let Some(previous) = revision {
            if previous != current {
                stats.mixed.lock().unwrap().push(format!(
                    "connection {connection} served revisions {previous} and {current}"
                ));
            }
        } else {
            revision = Some(current);
            stats.revisions.lock().unwrap().insert(current);
        }
        stats.requests.fetch_add(1, Ordering::Relaxed);

        let body = format!("proxied rev {current}");
        if write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
             Connection: keep-alive\r\n\r\n{body}",
            body.len()
        )
        .and_then(|()| stream.flush())
        .is_err()
        {
            break;
        }
    }

    stats.active.fetch_sub(1, Ordering::Relaxed);
    stats.closed.fetch_add(1, Ordering::Relaxed);
}

fn header_value<'a>(head: &'a str, expected: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(expected).then_some(value.trim())
    })
}

fn read_request_head_or_end(stream: &mut TcpStream) -> io::Result<Option<String>> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(count) => request.extend_from_slice(&chunk[..count]),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                ) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(Some(String::from_utf8_lossy(&request).into_owned()));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "upstream request head was too large",
    ))
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
                access_log logs/access.log;
                client_body_temp_path temp/client-body;
                proxy_temp_path temp/proxy;

                upstream backend {{
                    server 127.0.0.1:{upstream_port};
                    keepalive 2;
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location = /proxied {{
                        proxy_pass http://backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                    }}
                }}
            }}
        "#
    )
}
