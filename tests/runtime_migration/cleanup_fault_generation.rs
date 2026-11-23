use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::disk_fault_support::{
    EIO, FaultServer, FaultSpec, compiler_available, wait_for_no_files,
};
use super::support::{TestDirectory, assert_status, fake_upstream, request, reserve_port};

const OLD_BODY_SIZE: usize = 1024 * 1024;
const OLD_CHUNK_SIZE: usize = 16 * 1024;
const OLD_CHUNK_DELAY: Duration = Duration::from_millis(4);
const NEW_BODY: &str = "new-generation-cache-store";

#[test]
#[ignore = "requires local TCP bind and a C compiler; run with --ignored"]
fn old_cleanup_failures_do_not_cross_generations_after_reload_and_reset() {
    if compiler_available().is_none() {
        eprintln!("skipping cleanup fault generation migration: cc is not available");
        return;
    }

    let port = reserve_port();
    let Some(old_upstream) = SlowOneUpstream::start() else {
        return;
    };
    let Some(new_upstream) = fake_upstream(NEW_BODY) else {
        return;
    };
    let directory = TestDirectory::new("cleanup-fault-generation");
    let old_config = config(port, old_upstream.port, new_upstream.port, 0);
    let Some(mut server) = FaultServer::start(
        directory,
        port,
        &old_config,
        FaultSpec {
            path: "/state-old/",
            error: EIO,
            operations: "write,unlink,close",
        },
    ) else {
        return;
    };

    let mut client = TcpStream::connect(("127.0.0.1", port)).expect("connect cleanup client");
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(4096)
        .unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    client
        .write_all(b"GET /item/old HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let mut prefix = [0_u8; 4096];
    let read = client.read(&mut prefix).expect("read old response prefix");
    assert!(read > 0, "old generation response had no prefix");
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);

    wait_for_old_sink_files(&server.temp_path("state-old"));

    let new_config = config(port, old_upstream.port, new_upstream.port, 1);
    server.write_config(&new_config);
    server.reload();
    wait_for_version(port);

    let first = request(port, "/item/after");
    assert_status(&first, 200);
    assert_eq!(first_body(&first), NEW_BODY);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "new generation request should fill its cache:\n{first}"
    );
    wait_for_store(&server.temp_path("state-new/html/item/after"), NEW_BODY);

    let second = request(port, "/item/after");
    assert_status(&second, 200);
    assert_eq!(first_body(&second), NEW_BODY);
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "new generation cache was polluted by old cleanup:\n{second}"
    );
    new_upstream.captured();

    wait_for_no_files(&server.temp_path("temp-new/proxy"), "new response temp");
    wait_for_no_files(
        &server.temp_path("temp-new/client-body"),
        "new request temp",
    );
    assert!(
        temporary_sinks(&server.temp_path("state-new")).is_empty(),
        "old cleanup touched the new generation state: {:?}",
        temporary_sinks(&server.temp_path("state-new"))
    );
    assert!(
        !temporary_sinks(&server.temp_path("state-old")).is_empty(),
        "injected unlink failures should leave old-generation cleanup residue"
    );

    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    assert!(
        !temporary_sinks(&server.temp_path("state-old")).is_empty(),
        "shutdown unexpectedly removed residue hidden by the unlink fault"
    );
    assert!(
        temporary_sinks(&server.temp_path("state-new")).is_empty(),
        "shutdown removed or polluted the wrong generation state"
    );
}

fn wait_for_old_sink_files(root: &Path) {
    wait_until(
        Duration::from_secs(10),
        "old cache/store temp files",
        || temporary_sinks(root).len() >= 2,
    );
}

fn wait_for_store(path: &Path, body: &str) {
    wait_until(Duration::from_secs(10), "new store publication", || {
        fs::read(path).is_ok_and(|contents| contents == body.as_bytes())
    });
}

fn wait_for_version(port: u16) {
    wait_until(Duration::from_secs(5), "generation reload", || {
        request(port, "/version").ends_with("rev-1")
    });
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn first_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").expect("response headers").1
}

fn temporary_sinks(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(".starling-cache-") || name.starts_with(".starling-store-")
                })
        })
        .collect()
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else {
            files.push(path);
        }
    }
    files
}

struct SlowOneUpstream {
    port: u16,
}

impl SlowOneUpstream {
    fn start() -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping cleanup fault generation test: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind cleanup slow upstream: {error}"),
        };
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept cleanup slow request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            read_request_head(&mut stream).expect("read cleanup slow request");
            if let Err(error) = write_slow_response(&mut stream)
                && !matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                )
            {
                panic!("write cleanup slow response: {error}");
            }
        });
        Some(Self { port })
    }
}

fn read_request_head(stream: &mut TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while request.len() < 64 * 1024 {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..count]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(());
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "upstream request head was incomplete or too large",
    ))
}

fn write_slow_response(stream: &mut TcpStream) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
         Content-Length: {OLD_BODY_SIZE}\r\nConnection: close\r\n\r\n"
    )?;
    stream.flush()?;
    let chunk = vec![b'o'; OLD_CHUNK_SIZE];
    for _ in 0..OLD_BODY_SIZE / OLD_CHUNK_SIZE {
        stream.write_all(&chunk)?;
        thread::sleep(OLD_CHUNK_DELAY);
    }
    stream.flush()
}

fn config(port: u16, old_upstream_port: u16, new_upstream_port: u16, revision: u64) -> String {
    let generation = if revision == 0 { "old" } else { "new" };
    let upstream_port = if revision == 0 {
        old_upstream_port
    } else {
        new_upstream_port
    };
    format!(
        r#"
            daemon off;
            worker_processes 1;
            worker_shutdown_timeout 500ms;
            pid logs/nginx.pid;
            error_log stderr notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log off;
                client_body_temp_path temp-{generation}/client-body;
                proxy_temp_path temp-{generation}/proxy;
                proxy_cache_path state-{generation}/cache levels=1:2
                                 keys_zone=cleanup_fault_{generation}:1m inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root state-{generation}/html;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location /item/ {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_buffering off;
                        proxy_ignore_client_abort on;
                        proxy_cache cleanup_fault_{generation};
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        proxy_read_timeout 10s;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}
