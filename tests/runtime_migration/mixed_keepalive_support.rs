use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub(super) const BODY_SIZE: usize = 1024 * 1024;
const CHUNK_SIZE: usize = 8 * 1024;
const CHUNK_DELAY: Duration = Duration::from_millis(2);

#[derive(Clone, Debug)]
pub(super) struct Observation {
    pub(super) connection: usize,
    pub(super) path: String,
    pub(super) revision: u64,
}

#[derive(Clone)]
pub(super) struct Stats {
    next_connection: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    reset_writes: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<Observation>>>,
    mixed_revisions: Arc<Mutex<Vec<String>>>,
}

impl Stats {
    fn new() -> Self {
        Self {
            next_connection: Arc::new(AtomicUsize::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            reset_writes: Arc::new(AtomicUsize::new(0)),
            observations: Arc::new(Mutex::new(Vec::new())),
            mixed_revisions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(super) fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    pub(super) fn observations(&self) -> Vec<Observation> {
        self.observations.lock().unwrap().clone()
    }

    pub(super) fn reset_writes(&self) -> usize {
        self.reset_writes.load(Ordering::Relaxed)
    }

    pub(super) fn mixed_revisions(&self) -> Vec<String> {
        self.mixed_revisions.lock().unwrap().clone()
    }
}

pub(super) struct MixedUpstream {
    pub(super) port: u16,
    pub(super) stats: Stats,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl MixedUpstream {
    pub(super) fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping mixed keepalive/cache test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind mixed keepalive upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let stats = Stats::new();
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

    pub(super) fn active(&self) -> usize {
        self.stats.active()
    }

    pub(super) fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(listener: TcpListener, stats: Stats, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let stats = stats.clone();
                thread::spawn(move || serve_connection(stream, stats));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("mixed keepalive accept failed: {error}"),
        }
    }
}

fn serve_connection(mut stream: TcpStream, stats: Stats) {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let connection = stats.next_connection.fetch_add(1, Ordering::Relaxed) + 1;
    stats.active.fetch_add(1, Ordering::Relaxed);
    let mut revision = None;

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
            Err(error) => panic!("mixed keepalive request failed: {error}"),
        };
        let path = request_path(&request);
        let current = header_value(&request, "x-config-revision")
            .and_then(|value| value.parse::<u64>().ok())
            .expect("proxied request must carry X-Config-Revision");
        if let Some(previous) = revision {
            if previous != current {
                stats.mixed_revisions.lock().unwrap().push(format!(
                    "connection {connection} served revisions {previous} and {current}"
                ));
            }
        } else {
            revision = Some(current);
        }
        stats.observations.lock().unwrap().push(Observation {
            connection,
            path,
            revision: current,
        });

        if write_slow_response(&mut stream, connection).is_err() {
            stats.reset_writes.fetch_add(1, Ordering::Relaxed);
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
                "upstream request head too large",
            ));
        }
    }
}

fn request_path(request: &str) -> String {
    request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .expect("upstream request target")
        .to_owned()
}

fn header_value<'a>(head: &'a str, expected: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case(expected).then_some(value.trim())
    })
}

fn write_slow_response(stream: &mut TcpStream, connection: usize) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {BODY_SIZE}\r\nConnection: keep-alive\r\n\
         X-Upstream-Connection: {connection}\r\n\r\n"
    )?;
    stream.flush()?;
    let chunk = vec![b'x'; CHUNK_SIZE];
    for _ in 0..BODY_SIZE / CHUNK_SIZE {
        stream.write_all(&chunk)?;
        thread::sleep(CHUNK_DELAY);
    }
    stream.flush()
}
