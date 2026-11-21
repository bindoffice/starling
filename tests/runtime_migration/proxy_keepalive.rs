use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn reuses_one_named_upstream_keepalive_connection_for_two_requests() {
    let port = reserve_port();
    let Some(upstream) = fake_keepalive_upstream() else {
        return;
    };
    let directory = TestDirectory::new("proxy-keepalive");
    let config = proxy_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let first = server.get("/one");
    assert_status(&first, 200);
    assert!(
        first.contains("upstream-one"),
        "first upstream response missing:\n{first}"
    );

    let second = server.get("/two");
    assert_status(&second, 200);
    assert!(
        second.contains("upstream-two"),
        "second upstream response missing:\n{second}"
    );

    let report = upstream.finish();
    assert_eq!(report.connections, 1, "{report:?}");
    assert_eq!(report.requests.len(), 2, "{report:?}");
    assert!(
        report.requests[0].starts_with("GET /one HTTP/1.1\r\n"),
        "{report:?}"
    );
    assert!(
        report.requests[1].starts_with("GET /two HTTP/1.1\r\n"),
        "{report:?}"
    );
}

fn proxy_config(port: u16, upstream_port: u16) -> String {
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

                upstream runtime_backend {{
                    server 127.0.0.1:{upstream_port};
                    keepalive 2;
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location / {{
                        proxy_pass http://runtime_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                    }}
                }}
            }}
        "#
    )
}

#[derive(Debug)]
struct UpstreamReport {
    connections: usize,
    requests: Vec<String>,
}

struct FakeKeepaliveUpstream {
    port: u16,
    report: Receiver<Result<UpstreamReport, String>>,
}

impl FakeKeepaliveUpstream {
    fn finish(self) -> UpstreamReport {
        self.report
            .recv_timeout(Duration::from_secs(5))
            .expect("upstream keepalive report timed out")
            .unwrap_or_else(|error| panic!("fake keepalive upstream failed: {error}"))
    }
}

fn fake_keepalive_upstream() -> Option<FakeKeepaliveUpstream> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime proxy keepalive test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind fake keepalive upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (sender, report) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_two_requests(listener);
        let _ = sender.send(result.map_err(|error| error.to_string()));
    });
    Some(FakeKeepaliveUpstream { port, report })
}

fn serve_two_requests(listener: TcpListener) -> io::Result<UpstreamReport> {
    let (mut first_stream, _) = listener.accept()?;
    configure_stream(&first_stream)?;
    let first = read_request_head(&mut first_stream)?;
    write_upstream_response(&mut first_stream, &first)?;

    first_stream.set_read_timeout(Some(Duration::from_millis(800)))?;
    match read_request_head(&mut first_stream) {
        Ok(second) => {
            write_upstream_response(&mut first_stream, &second)?;
            Ok(UpstreamReport {
                connections: 1,
                requests: vec![first, second],
            })
        }
        Err(error) if is_read_timeout_or_close(&error) => {
            first_stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let (mut second_stream, _) = listener.accept()?;
            configure_stream(&second_stream)?;
            let second = read_request_head(&mut second_stream)?;
            write_upstream_response(&mut second_stream, &second)?;
            Ok(UpstreamReport {
                connections: 2,
                requests: vec![first, second],
            })
        }
        Err(error) => Err(error),
    }
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut byte = [0_u8; 1];
    while request.len() < 64 * 1024 {
        match stream.read(&mut byte) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "upstream client closed before request head",
                ));
            }
            Ok(_) => request.push(byte[0]),
            Err(error) => return Err(error),
        }
        if request.ends_with(b"\r\n\r\n") {
            return Ok(String::from_utf8_lossy(&request).into_owned());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "upstream request head too large",
    ))
}

fn write_upstream_response(stream: &mut TcpStream, request: &str) -> io::Result<()> {
    let body = if request.starts_with("GET /one ") {
        "upstream-one"
    } else {
        "upstream-two"
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         Connection: keep-alive\r\n\
         \r\n{}",
        body.len(),
        body
    )?;
    stream.flush()
}

fn is_read_timeout_or_close(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::UnexpectedEof
    )
}
