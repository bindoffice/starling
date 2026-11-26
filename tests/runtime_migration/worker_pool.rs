use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{Http1Connection, RunningServer, TestDirectory, assert_status, reserve_port};

const WORKERS: usize = 8;
const CLIENTS: usize = 12;
const RELOADS: usize = 8;

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn multiple_workers_share_listeners_reload_and_graceful_shutdown() {
    let port = reserve_port();
    let stream_port = reserve_port();
    let directory = TestDirectory::new("worker-pool");
    let unix_socket = directory.path().join("logs/http.sock");
    let mut server =
        RunningServer::start(directory, port, &config(port, stream_port, &unix_socket, 0));
    wait_for_worker_threads(server.pid(), WORKERS);

    let mut control = Http1Connection::connect(port);
    wait_for_version(&mut control, "worker-v0");
    assert_eq!(request_unix(&unix_socket, "/version"), "worker-v0");
    assert_eq!(request_stream(stream_port), "stream-ok");

    let stop = Arc::new(AtomicBool::new(false));
    let mut clients = Vec::new();
    for _ in 0..CLIENTS {
        let stop = stop.clone();
        clients.push(thread::spawn(move || {
            let mut connection = Http1Connection::connect(port);
            let mut completed = 0_usize;
            while !stop.load(Ordering::Relaxed) {
                let response = connection.get("/version");
                assert_status(&response, 200);
                assert!(
                    response.contains("worker-v"),
                    "unexpected response during worker reload:\n{response}"
                );
                completed += 1;
            }
            completed
        }));
    }

    for revision in 1..=RELOADS {
        server.write_config(&config(port, stream_port, &unix_socket, revision));
        server.reload();
        wait_for_version(&mut control, &format!("worker-v{revision}"));
    }
    stop.store(true, Ordering::Relaxed);

    let completed = clients
        .into_iter()
        .map(|client| client.join().expect("worker keepalive client panicked"))
        .sum::<usize>();
    assert!(
        completed >= CLIENTS,
        "multi-worker traffic did not complete enough requests: {completed}"
    );
    server.assert_running();

    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    assert!(
        !unix_socket.exists(),
        "worker shutdown left the UNIX listen socket behind"
    );
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

fn wait_for_worker_threads(pid: u32, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let count = worker_thread_count(pid);
        if count == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "expected {expected} worker threads, found {count}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn worker_thread_count(pid: u32) -> usize {
    let Ok(tasks) = fs::read_dir(format!("/proc/{pid}/task")) else {
        return 0;
    };
    tasks
        .filter_map(Result::ok)
        .filter(|task| {
            fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim_start().starts_with("starling-worker"))
        })
        .count()
}

fn request_unix(path: &Path, target: &str) -> String {
    let mut stream = UnixStream::connect(path).expect("connect UNIX HTTP listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read UNIX HTTP response");
    let response = String::from_utf8(response).expect("UNIX HTTP response is UTF-8");
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default()
        .to_string()
}

fn request_stream(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect stream listener");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read stream response");
    String::from_utf8(response).expect("stream response is UTF-8")
}

fn config(port: u16, stream_port: u16, unix_socket: &Path, revision: usize) -> String {
    let unix_socket = unix_socket.display();
    format!(
        r#"
            daemon off;
            worker_processes {WORKERS};
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 256;
            }}

            http {{
                default_type text/plain;
                access_log off;
                keepalive_requests 100000000;
                keepalive_timeout 300s;

                server {{
                    listen 127.0.0.1:{port};
                    listen unix:{unix_socket};
                    server_name localhost;

                    location = /version {{
                        return 200 "worker-v{revision}";
                    }}
                }}
            }}

            stream {{
                server {{
                    listen 127.0.0.1:{stream_port};
                    return "stream-ok";
                }}
            }}
        "#
    )
}
