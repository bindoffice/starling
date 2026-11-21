use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, request, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn refreshes_expired_cache_in_background_while_serving_stale() {
    let port = reserve_port();
    let Some(upstream) = background_upstream() else {
        return;
    };
    let directory = TestDirectory::new("cache-background-update");
    let config = background_update_config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let first = server.get("/item");
    assert_status(&first, 200);
    assert_cache_status(&first, "miss");
    assert!(
        first.ends_with("old"),
        "unexpected first response:\n{first}"
    );

    thread::sleep(Duration::from_millis(1100));

    let (stale_sender, stale_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = stale_sender.send(request(port, "/item"));
    });
    upstream
        .update_started
        .recv_timeout(Duration::from_secs(5))
        .expect("expired cache entry did not start a background update");
    let stale = stale_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("stale response waited for the background cache update");
    assert_status(&stale, 200);
    assert_cache_status(&stale, "stale");
    assert!(
        stale.ends_with("old"),
        "unexpected stale response:\n{stale}"
    );

    let updating = server.get("/item");
    assert_status(&updating, 200);
    assert_cache_status(&updating, "updating");
    assert!(
        updating.ends_with("old"),
        "unexpected updating response:\n{updating}"
    );

    upstream
        .release_update
        .send(())
        .expect("release background cache update");
    let captured = upstream
        .finished
        .recv_timeout(Duration::from_secs(5))
        .expect("background cache update did not finish")
        .expect("background cache update failed");
    assert!(
        captured.0.starts_with("GET /item HTTP/1.1\r\n"),
        "unexpected cache-fill request:\n{}",
        captured.0
    );
    assert!(
        captured.1.starts_with("GET /item HTTP/1.1\r\n"),
        "unexpected background-update request:\n{}",
        captured.1
    );

    let mut last = String::new();
    for _ in 0..100 {
        thread::sleep(Duration::from_millis(20));
        last = server.get("/item");
        if last.to_ascii_lowercase().contains("x-cache-status: hit") {
            break;
        }
    }
    assert_status(&last, 200);
    assert_cache_status(&last, "hit");
    assert!(
        last.ends_with("new"),
        "background update did not replace the cached body:\n{last}"
    );
    server.assert_running();
}

fn assert_cache_status(response: &str, expected: &str) {
    assert!(
        response
            .to_ascii_lowercase()
            .contains(&format!("x-cache-status: {expected}\r\n")),
        "expected cache status {expected}:\n{response}"
    );
}

struct BackgroundUpstream {
    port: u16,
    update_started: Receiver<()>,
    release_update: Sender<()>,
    finished: Receiver<Result<(String, String), String>>,
}

fn background_upstream() -> Option<BackgroundUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping runtime cache background update test: TCP binding is not permitted"
            );
            return None;
        }
        Err(error) => panic!("bind background upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (update_started, update_started_rx) = mpsc::channel();
    let (release_update, release_update_rx) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_background_update(listener, update_started, release_update_rx);
        let _ = finished.send(result.map_err(|error| error.to_string()));
    });
    Some(BackgroundUpstream {
        port,
        update_started: update_started_rx,
        release_update,
        finished: finished_rx,
    })
}

fn serve_background_update(
    listener: TcpListener,
    update_started: Sender<()>,
    release_update: Receiver<()>,
) -> io::Result<(String, String)> {
    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let first_request = read_request_head(&mut stream)?;
    write_response(&mut stream, "old")?;

    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let second_request = read_request_head(&mut stream)?;
    update_started
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "background test exited"))?;
    release_update
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "background update was not released",
            )
        })?;
    write_response(&mut stream, "new")?;
    Ok((first_request, second_request))
}

fn write_response(stream: &mut TcpStream, body: &str) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n{}",
        body.len(),
        body
    )?;
    stream.flush()
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

fn background_update_config(port: u16, upstream_port: u16) -> String {
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
                        proxy_cache_valid 200 1s;
                        proxy_cache_use_stale updating;
                        proxy_cache_background_update on;
                        proxy_cache_lock on;
                    }}
                }}
            }}
        "#
    )
}
