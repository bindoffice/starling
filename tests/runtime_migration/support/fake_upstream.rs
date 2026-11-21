use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

pub(crate) struct FakeUpstream {
    pub(crate) port: u16,
    captured: Receiver<Result<String, String>>,
}

impl FakeUpstream {
    pub(crate) fn captured(self) -> String {
        self.captured
            .recv_timeout(Duration::from_secs(5))
            .expect("upstream request capture timed out")
            .unwrap_or_else(|error| panic!("fake upstream failed: {error}"))
    }
}

pub(crate) fn fake_upstream(body: &str) -> Option<FakeUpstream> {
    fake_upstream_with_status(body, 200)
}

pub(crate) fn fake_upstream_with_status(body: &str, status: u16) -> Option<FakeUpstream> {
    fake_upstream_full(body, status, Duration::ZERO)
}

pub(crate) fn fake_upstream_with_delay(body: &str, delay: Duration) -> Option<FakeUpstream> {
    fake_upstream_full(body, 200, delay)
}

fn fake_upstream_full(body: &str, status: u16, delay: Duration) -> Option<FakeUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime migration proxy test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind fake upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let body = body.to_owned();
    let (sender, captured) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_one(listener, &body, status, delay);
        let _ = sender.send(result.map_err(|error| error.to_string()));
    });
    Some(FakeUpstream { port, captured })
}

fn serve_one(
    listener: TcpListener,
    body: &str,
    status: u16,
    delay: Duration,
) -> io::Result<String> {
    let (mut stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = read_request_head(&mut stream)?;
    thread::sleep(delay);
    write!(
        stream,
        "HTTP/1.1 {status} {}\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         X-Upstream: fake\r\n\
         Connection: close\r\n\
         \r\n{}",
        reason_phrase(status),
        body.len(),
        body
    )?;
    stream.flush()?;
    Ok(request)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Test Status",
    }
}

pub(crate) fn read_request_head(stream: &mut TcpStream) -> io::Result<String> {
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
