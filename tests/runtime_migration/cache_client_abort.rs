use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::support::{
    RunningServer, TestDirectory, assert_status, read_request_head, request, reserve_port,
};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn cache_fill_survives_client_reset_and_releases_the_lock() {
    let port = reserve_port();
    let Some(upstream) = delayed_upstream() else {
        return;
    };
    let directory = TestDirectory::new("cache-client-abort");
    let config = abort_config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let mut aborted = TcpStream::connect(("127.0.0.1", port)).expect("connect aborted client");
    aborted
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    aborted
        .write_all(b"GET /abort HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    read_until(&mut aborted, b"first");
    upstream
        .request_seen
        .recv_timeout(Duration::from_secs(5))
        .expect("cache fill did not reach upstream");
    reset_client(aborted);

    let (second_sender, second_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = second_sender.send(request(port, "/abort"));
    });
    if let Ok(response) = second_receiver.recv_timeout(Duration::from_millis(250)) {
        panic!("second client completed before the cache fill finished:\n{response}");
    }

    upstream
        .release
        .send(())
        .expect("release delayed upstream response");
    let captured = upstream
        .finished
        .recv_timeout(Duration::from_secs(5))
        .expect("aborted cache fill did not finish")
        .expect("aborted cache fill failed");
    assert!(
        captured.starts_with("GET /abort HTTP/1.1\r\n"),
        "unexpected upstream request:\n{captured}"
    );

    let second = second_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("second client did not receive the cached response");
    assert_status(&second, 200);
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "second client did not hit the completed cache entry:\n{second}"
    );
    assert!(second.ends_with("firstsecond"), "{second}");
    server.assert_running();
}

fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
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
            "server closed before sending the first body chunk"
        );
        response.extend_from_slice(&chunk[..count]);
    }
}

struct DelayedUpstream {
    port: u16,
    request_seen: Receiver<()>,
    release: Sender<()>,
    finished: Receiver<Result<String, String>>,
}

fn delayed_upstream() -> Option<DelayedUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime cache client abort test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind delayed upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (request_seen, request_seen_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_delayed(listener, request_seen, release_rx);
        let _ = finished.send(result.map_err(|error| error.to_string()));
    });
    Some(DelayedUpstream {
        port,
        request_seen: request_seen_rx,
        release,
        finished: finished_rx,
    })
}

fn serve_delayed(
    listener: TcpListener,
    request_seen: Sender<()>,
    release: Receiver<()>,
) -> io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = read_request_head(&mut stream)?;
    stream.write_all(
        b"HTTP/1.1 200 OK\r\n\
          Content-Type: text/plain\r\n\
          Content-Length: 11\r\n\
          Connection: close\r\n\
          \r\n\
          first",
    )?;
    stream.flush()?;
    request_seen
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "client abort test exited"))?;
    release.recv_timeout(Duration::from_secs(5)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "aborted cache fill was not released",
        )
    })?;
    stream.write_all(b"second")?;
    stream.flush()?;
    Ok(request)
}

fn abort_config(port: u16, upstream_port: u16) -> String {
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
                        proxy_buffering off;
                        proxy_cache runtime_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_cache_lock on;
                        proxy_cache_lock_timeout 5s;
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}
