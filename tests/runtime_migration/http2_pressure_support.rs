use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub(super) const BODY_SIZE: usize = 256 * 1024;
const CHUNK_SIZE: usize = 16 * 1024;
const CHUNK_DELAY: Duration = Duration::from_millis(2);

#[derive(Clone)]
pub(super) struct PressureStats {
    opened: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    requests: Arc<AtomicUsize>,
    reused: Arc<AtomicUsize>,
    revisions: Arc<Mutex<HashSet<u64>>>,
    mixed: Arc<Mutex<Vec<String>>>,
}

impl PressureStats {
    fn new() -> Self {
        Self {
            opened: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            requests: Arc::new(AtomicUsize::new(0)),
            reused: Arc::new(AtomicUsize::new(0)),
            revisions: Arc::new(Mutex::new(HashSet::new())),
            mixed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn requests(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    pub(super) fn reused(&self) -> usize {
        self.reused.load(Ordering::Relaxed)
    }

    pub(super) fn revisions(&self) -> HashSet<u64> {
        self.revisions.lock().unwrap().clone()
    }

    pub(super) fn mixed_revisions(&self) -> Vec<String> {
        self.mixed.lock().unwrap().clone()
    }
}

pub(super) struct PressureUpstream {
    pub(super) port: u16,
    pub(super) stats: PressureStats,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl PressureUpstream {
    pub(super) fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping HTTP/2 pressure test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind HTTP/2 pressure upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stats = PressureStats::new();
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

    pub(super) fn wait_for_requests(&self, expected: usize, timeout: Duration) {
        wait_until(timeout, "HTTP/2 pressure upstream requests", || {
            self.stats.requests() >= expected
        });
    }

    pub(super) fn wait_for_no_active(&self, timeout: Duration) {
        wait_until(timeout, "HTTP/2 pressure upstream drain", || {
            self.stats.active.load(Ordering::Relaxed) == 0
        });
    }

    pub(super) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(listener: TcpListener, stats: PressureStats, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let stats = stats.clone();
                thread::spawn(move || serve_connection(stream, stats));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("HTTP/2 pressure accept failed: {error}"),
        }
    }
}

fn serve_connection(mut stream: TcpStream, stats: PressureStats) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let connection = stats.opened.fetch_add(1, Ordering::Relaxed) + 1;
    stats.active.fetch_add(1, Ordering::Relaxed);
    let mut revision = None;
    let mut served = 0_usize;
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
            Err(_) => break,
        };
        let current = header_value(&request, "x-config-revision")
            .and_then(|value| value.parse::<u64>().ok())
            .expect("pressure request must include X-Config-Revision");
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
        served += 1;
        if served > 1 {
            stats.reused.fetch_add(1, Ordering::Relaxed);
        }
        stats.requests.fetch_add(1, Ordering::Relaxed);
        let result = if request.starts_with("GET /slow/") {
            write_slow_response(&mut stream)
        } else {
            write_probe_response(&mut stream, current)
        };
        if result.is_err() {
            break;
        }
    }
    stats.active.fetch_sub(1, Ordering::Relaxed);
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

fn write_probe_response(stream: &mut TcpStream, revision: u64) -> io::Result<()> {
    let body = format!("probe rev {revision}");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
         Connection: keep-alive\r\n\r\n{body}",
        body.len()
    )?;
    stream.flush()
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
                "pressure request head was too large",
            ));
        }
    }
}

fn header_value<'a>(head: &'a str, expected: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(expected).then_some(value.trim())
    })
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}
