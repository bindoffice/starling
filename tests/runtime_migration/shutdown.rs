use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, read_request_head, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn worker_shutdown_timeout_forces_graceful_shutdown() {
    let port = reserve_port();
    let Some(upstream) = stalled_upstream() else {
        return;
    };
    let directory = TestDirectory::new("worker-shutdown-timeout");
    let config = shutdown_config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect streaming client");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    client
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    read_until(&mut client, b"first");
    upstream
        .request_seen
        .recv_timeout(Duration::from_secs(5))
        .expect("streaming request did not reach upstream");

    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
}

fn read_until(stream: &mut TcpStream, needle: &[u8]) {
    let mut response = Vec::new();
    while !response
        .windows(needle.len())
        .any(|window| window == needle)
    {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).expect("read streaming response");
        assert!(
            count > 0,
            "server closed before the upstream first chunk arrived"
        );
        response.extend_from_slice(&chunk[..count]);
    }
}

struct StalledUpstream {
    port: u16,
    request_seen: Receiver<()>,
}

fn stalled_upstream() -> Option<StalledUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime worker shutdown test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind stalled upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (request_seen, request_seen_rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = serve_stalled(listener, request_seen);
    });
    Some(StalledUpstream {
        port,
        request_seen: request_seen_rx,
    })
}

fn serve_stalled(listener: TcpListener, request_seen: mpsc::Sender<()>) -> io::Result<()> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let _ = read_request_head(&mut stream)?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: text/plain\r\n\
          Transfer-Encoding: chunked\r\n\
          Connection: close\r\n\
          \r\n\
          5\r\nfirst\r\n",
    )?;
    stream.flush()?;
    let _ = request_seen.send(());
    thread::sleep(Duration::from_secs(5));
    Ok(())
}

fn shutdown_config(port: u16, upstream_port: u16) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            worker_shutdown_timeout 100ms;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 128;
            }}

            http {{
                access_log logs/access.log;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location / {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffering off;
                        proxy_read_timeout 10s;
                    }}
                }}
            }}
        "#
    )
}
