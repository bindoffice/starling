use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn cache_lock_coalesces_concurrent_misses_from_real_clients() {
    let port = reserve_port();
    let Some(upstream) = delayed_upstream() else {
        return;
    };
    let directory = TestDirectory::new("cache-lock");
    let config = cache_lock_config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let first = thread::spawn(move || request(port, "/locked"));
    upstream
        .accepted
        .recv_timeout(Duration::from_secs(5))
        .expect("first cache miss did not reach upstream");

    let second = thread::spawn(move || request(port, "/locked"));
    thread::sleep(Duration::from_millis(100));
    upstream
        .release
        .send(())
        .expect("release delayed upstream response");

    let first = first.join().expect("join first cache-lock client");
    let second = second.join().expect("join second cache-lock client");
    assert_status(&first, 200);
    assert_status(&second, 200);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "lock owner was not a cache miss:\n{first}"
    );
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "waiting client did not observe the cached response:\n{second}"
    );
    assert!(first.contains("cache-lock-body"), "{first}");
    assert!(second.contains("cache-lock-body"), "{second}");

    let captured = upstream
        .request
        .recv_timeout(Duration::from_secs(5))
        .expect("missing upstream request capture")
        .expect("upstream request capture failed");
    assert!(
        captured.starts_with("GET /locked HTTP/1.1\r\n"),
        "unexpected upstream request:\n{captured}"
    );
    server.assert_running();
}

struct DelayedUpstream {
    port: u16,
    accepted: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    request: mpsc::Receiver<Result<String, String>>,
}

fn delayed_upstream() -> Option<DelayedUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime cache lock test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind delayed upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (accepted, accepted_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (request, request_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_delayed(listener, accepted, release_rx);
        let _ = request.send(result.map_err(|error| error.to_string()));
    });
    Some(DelayedUpstream {
        port,
        accepted: accepted_rx,
        release,
        request: request_rx,
    })
}

fn serve_delayed(
    listener: TcpListener,
    accepted: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
) -> io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let request = read_request_head(&mut stream)?;
    accepted
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "cache lock test disappeared"))?;
    release.recv_timeout(Duration::from_secs(5)).map_err(|_| {
        io::Error::new(io::ErrorKind::TimedOut, "delayed upstream was not released")
    })?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: 15\r\n\
         Connection: close\r\n\
         \r\n\
         cache-lock-body"
    )?;
    stream.flush()?;
    Ok(request)
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&request).into_owned());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "upstream request head was incomplete or too large",
    ))
}

fn cache_lock_config(port: u16, upstream_port: u16) -> String {
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
                proxy_cache_path cache levels=1:2 keys_zone=runtime_cache:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    add_header X-Cache-Status $upstream_cache_status always;

                    location / {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_cache runtime_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_cache_lock on;
                        proxy_cache_lock_timeout 2s;
                        proxy_cache_lock_age 1s;
                    }}
                }}
            }}
        "#
    )
}
