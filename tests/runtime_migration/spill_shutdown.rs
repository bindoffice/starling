use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use super::support::{RunningServer, TestDirectory, read_request_head, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn sigquit_and_client_reset_clean_an_inflight_response_spill() {
    let port = reserve_port();
    let Some(upstream) = spill_upstream() else {
        return;
    };
    let directory = TestDirectory::new("sigquit-spill");
    let prefix = directory.path().to_path_buf();
    let config = config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect slow spill client");
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_files(&temp_root, true);
    upstream
        .started
        .recv_timeout(Duration::from_secs(5))
        .expect("upstream did not start the spilled response");

    server.shutdown_gracefully();
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);

    server.wait_for_exit(Duration::from_secs(5));
    assert!(
        upstream
            .closed
            .recv_timeout(Duration::from_secs(2))
            .expect("upstream connection was not closed during shutdown")
    );
    wait_for_files(&temp_root, false);
}

struct SpillUpstream {
    port: u16,
    started: Receiver<()>,
    closed: Receiver<bool>,
}

fn spill_upstream() -> Option<SpillUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime shutdown spill test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind shutdown spill upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (started_sender, started) = mpsc::channel();
    let (closed_sender, closed) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_spill(listener, started_sender);
        let _ = closed_sender.send(result.unwrap_or(false));
    });
    Some(SpillUpstream {
        port,
        started,
        closed,
    })
}

fn serve_spill(listener: TcpListener, started: mpsc::Sender<()>) -> io::Result<bool> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    read_request_head(&mut stream)?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: application/octet-stream\r\n\
          Content-Length: 4194304\r\n\
          Connection: close\r\n\
          \r\n",
    )?;
    stream.write_all(&vec![b'x'; 128 * 1024])?;
    stream.flush()?;
    started
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "shutdown test exited"))?;

    let mut byte = [0_u8; 1];
    match stream.read(&mut byte) {
        Ok(0) => Ok(true),
        Ok(_) => Ok(false),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(true)
        }
        Err(error) => Err(error),
    }
}

fn wait_for_files(root: &std::path::Path, present: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !files_under(root).is_empty() == present {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for spill files under {} (present={present}): {:?}",
            root.display(),
            files_under(root)
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return files;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

fn config(port: u16, upstream_port: u16) -> String {
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
                default_type application/octet-stream;
                access_log logs/access.log;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    lingering_close off;

                    location /spill {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_buffer_size 1k;
                        proxy_buffers 4 1k;
                        proxy_busy_buffers_size 2k;
                        proxy_temp_file_write_size 1k;
                        proxy_temp_path proxy-temp;
                        proxy_max_temp_file_size 5m;
                        proxy_read_timeout 10s;
                    }}
                }}
            }}
        "#
    )
}
