use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::reload_stress_support::ProcessResources;
use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

const SSE_REQUESTS: usize = 10;
const CACHE_REQUESTS: usize = 12;
const CACHE_BODY_SIZE: usize = 128 * 1024;

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn sse_cache_and_store_runtime_curve_stays_bounded() {
    let Some(upstream) = StreamUpstream::start() else {
        return;
    };
    let port = reserve_port();
    let directory = TestDirectory::new("sse-cache-resource-curve");
    directory.write("temp/client-body/.keep", "");
    directory.write("temp/proxy/.keep", "");
    fs::create_dir_all(directory.path().join("html/cache")).unwrap();
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port));
    let baseline = ProcessResources::capture(server.pid());
    let mut samples = Vec::new();

    for index in 0..SSE_REQUESTS {
        let response = request_streaming(port, &format!("/sse/{index}"));
        assert_status(&response, 200);
        assert!(
            response.contains("data: 0\n\n") && response.contains("data: 4\n\n"),
            "SSE response did not contain all events:\n{response}"
        );
        if index % 2 == 0 {
            capture_sample(server.pid(), "sse", index, &mut samples);
        }
    }

    for index in 0..CACHE_REQUESTS {
        let response = server.get("/cache/item");
        assert_status(&response, 200);
        assert!(
            response.ends_with(&"x".repeat(CACHE_BODY_SIZE)),
            "cache response body was incomplete at request {index}"
        );
        let expected = if index == 0 { "miss" } else { "hit" };
        assert!(
            response
                .to_ascii_lowercase()
                .contains(&format!("x-cache-status: {expected}\r\n")),
            "unexpected cache status at request {index}:\n{}",
            response.lines().take(8).collect::<Vec<_>>().join("\n")
        );
        if index % 3 == 0 {
            capture_sample(server.pid(), "cache", index, &mut samples);
        }
    }

    wait_until(Duration::from_secs(5), "proxy_store publication", || {
        fs::metadata(server.directory_path().join("html/cache/item"))
            .is_ok_and(|metadata| metadata.len() == CACHE_BODY_SIZE as u64)
    });
    wait_until(Duration::from_secs(5), "SSE/cache temp cleanup", || {
        files_under(&server.directory_path().join("temp")).is_empty()
    });
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
            final_resources.fds <= baseline.fds + 8,
            "SSE/cache curve leaked FDs: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_tasks <= baseline.tasks + 32,
            "SSE/cache curve created too many tasks: baseline={baseline:?} peak={peak_tasks}"
        );
        assert!(
            final_resources.tasks <= baseline.tasks + 32,
            "SSE/cache tasks did not settle: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "SSE/cache curve leaked RSS: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_rss_kib <= baseline.rss_kib + 96 * 1024,
            "SSE/cache curve peaked too high: baseline={baseline:?} peak={peak_rss_kib} KiB"
        );
    }
    assert_eq!(
        upstream.requests.load(Ordering::Relaxed),
        SSE_REQUESTS + 1,
        "cache hits unexpectedly reached upstream"
    );
    assert_eq!(upstream.active.load(Ordering::Relaxed), 0);
    server.assert_running();
    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    upstream.stop();
}

fn capture_sample(pid: u32, phase: &str, index: usize, samples: &mut Vec<ProcessResources>) {
    if let Some(resources) = ProcessResources::capture(pid) {
        eprintln!(
            "SSE/cache resource sample phase={phase} request={index} fds={} tasks={} rss_kib={}",
            resources.fds, resources.tasks, resources.rss_kib
        );
        samples.push(resources);
    }
}

fn request_streaming(port: u16, target: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect SSE client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut output = Vec::new();
    stream.read_to_end(&mut output).unwrap();
    String::from_utf8_lossy(&output).into_owned()
}

fn files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else if path.file_name().is_none_or(|name| name != ".keep") {
            files.push(path);
        }
    }
    files
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn config(port: u16, upstream_port: u16) -> String {
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
                client_body_temp_path temp/client-body;
                proxy_temp_path temp/proxy;
                proxy_cache_path cache levels=1:2 keys_zone=stream_cache:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location /sse/ {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Connection "";
                        proxy_buffering off;
                        proxy_read_timeout 5s;
                    }}

                    location /cache/ {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Connection "";
                        proxy_buffer_size 1k;
                        proxy_buffers 4 4k;
                        proxy_busy_buffers_size 8k;
                        proxy_temp_file_write_size 4k;
                        proxy_max_temp_file_size 8m;
                        proxy_cache stream_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        proxy_read_timeout 5s;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}

struct StreamUpstream {
    port: u16,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl StreamUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping SSE/cache resource curve: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind SSE/cache upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = thread::spawn({
            let requests = requests.clone();
            let active = active.clone();
            let stop = stop.clone();
            move || accept_loop(listener, requests, active, stop)
        });
        Some(Self {
            port,
            requests,
            active,
            stop,
            accept: Some(accept),
        })
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let requests = requests.clone();
                let active = active.clone();
                thread::spawn(move || serve_connection(stream, requests, active));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("SSE/cache upstream accept failed: {error}"),
        }
    }
}

fn serve_connection(stream: TcpStream, requests: Arc<AtomicUsize>, active: Arc<AtomicUsize>) {
    let mut stream = stream;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    active.fetch_add(1, Ordering::Relaxed);
    loop {
        let request = match read_request_head(&mut stream) {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(error) if is_closed(&error) => break,
            Err(error) => panic!("SSE/cache upstream request failed: {error}"),
        };
        requests.fetch_add(1, Ordering::Relaxed);
        let result = if request.starts_with("GET /sse/") {
            write_sse(&mut stream)
        } else {
            write_cache_body(&mut stream)
        };
        if result.is_err() {
            break;
        }
    }
    active.fetch_sub(1, Ordering::Relaxed);
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
                "SSE/cache request head was too large",
            ));
        }
    }
}

fn write_sse(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
         Transfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n"
    )?;
    for index in 0..5 {
        let event = format!("data: {index}\n\n");
        write!(stream, "{:x}\r\n{event}\r\n", event.len())?;
        stream.flush()?;
        thread::sleep(Duration::from_millis(2));
    }
    write!(stream, "0\r\n\r\n")?;
    stream.flush()
}

fn write_cache_body(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {CACHE_BODY_SIZE}\r\nConnection: keep-alive\r\n\r\n"
    )?;
    let body = vec![b'x'; CACHE_BODY_SIZE];
    stream.write_all(&body)?;
    stream.flush()
}

fn is_closed(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
    )
}
