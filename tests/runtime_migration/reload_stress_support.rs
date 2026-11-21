use std::collections::HashSet;
use std::fs;
use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(super) struct ProcessResources {
    pub(super) fds: usize,
    pub(super) tasks: usize,
    pub(super) rss_kib: u64,
}

impl ProcessResources {
    pub(super) fn capture(pid: u32) -> Option<Self> {
        let fds = fs::read_dir(format!("/proc/{pid}/fd")).ok()?.count();
        let tasks = fs::read_dir(format!("/proc/{pid}/task")).ok()?.count();
        let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        let rss_kib = status.lines().find_map(|line| {
            line.strip_prefix("VmRSS:")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })?;
        Some(Self {
            fds,
            tasks,
            rss_kib,
        })
    }
}

#[derive(Clone)]
pub(super) struct UpstreamStats {
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

    pub(super) fn opened(&self) -> usize {
        self.opened.load(Ordering::Relaxed)
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub(super) fn revisions(&self) -> HashSet<u64> {
        self.revisions.lock().unwrap().clone()
    }

    pub(super) fn mixed_revisions(&self) -> Vec<String> {
        self.mixed.lock().unwrap().clone()
    }

    pub(super) fn summary(&self) -> String {
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

pub(super) struct StressUpstream {
    pub(super) port: u16,
    pub(super) stats: UpstreamStats,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl StressUpstream {
    pub(super) fn start() -> Option<Self> {
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

    pub(super) fn wait_for_no_active(&self, timeout: Duration) {
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

    pub(super) fn stop(mut self) {
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

fn serve_connection(mut stream: std::net::TcpStream, stats: UpstreamStats) {
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

fn read_request_head_or_end(stream: &mut std::net::TcpStream) -> io::Result<Option<String>> {
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

pub(super) fn config(port: u16, upstream_port: u16, revision: u64) -> String {
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
                keepalive_requests 100000000;
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
