use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::mixed_keepalive_support::{BODY_SIZE, MixedUpstream};
use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn half_close_reset_cache_store_and_reload_share_pooled_keepalive_correctly() {
    let port = reserve_port();
    let Some(upstream) = MixedUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("mixed-keepalive-cache");
    let root = directory.path().to_path_buf();
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));
    wait_for_version(&server, "rev-0");

    let half = request_half_closed(port, "/drain/half");
    assert_complete_response(&half);
    wait_for_store(&root, "drain/half");

    let reused = server.get("/drain/reuse");
    assert_status(&reused, 200);
    assert_eq!(response_body(&reused).len(), BODY_SIZE);
    wait_for_store(&root, "drain/reuse");

    request_reset(port, "/drain/reset");
    wait_for_store(&root, "drain/reset");

    request_reset(port, "/cancel/cancel");
    wait_until(Duration::from_secs(5), "canceled upstream write", || {
        upstream.stats.reset_writes() > 0
    });

    let probe = server.get("/cancel/probe");
    assert_status(&probe, 200);
    assert_eq!(response_body(&probe).len(), BODY_SIZE);

    server.write_config(&config(port, upstream.port, 1));
    server.reload();
    wait_for_version(&server, "rev-1");
    let after_reload = server.get("/drain/after-reload");
    assert_status(&after_reload, 200);
    assert_eq!(response_body(&after_reload).len(), BODY_SIZE);
    wait_for_store(&root, "drain/after-reload");

    let observations = upstream.stats.observations();
    let connection_for = |path: &str| {
        observations
            .iter()
            .find(|entry| entry.path == path)
            .unwrap_or_else(|| panic!("missing upstream observation for {path}: {observations:?}"))
            .connection
    };
    assert_eq!(
        connection_for("/drain/half"),
        connection_for("/drain/reuse"),
        "half-closed response did not return the upstream connection to the pool"
    );
    assert_eq!(
        connection_for("/drain/reuse"),
        connection_for("/drain/reset"),
        "cache/store drain after client reset did not reuse the pooled connection"
    );
    assert_eq!(
        connection_for("/drain/reset"),
        connection_for("/cancel/cancel"),
        "reset-cancel request did not start on the existing pooled connection"
    );
    assert_ne!(
        connection_for("/cancel/cancel"),
        connection_for("/cancel/probe"),
        "an upstream connection canceled by RST was reused"
    );
    assert_ne!(
        connection_for("/cancel/probe"),
        connection_for("/drain/after-reload"),
        "reload returned an old-generation upstream connection to the new pool"
    );
    assert!(
        upstream.stats.mixed_revisions().is_empty(),
        "upstream connection served more than one generation: {:?}",
        upstream.stats.mixed_revisions()
    );
    for observation in &observations {
        let expected = if observation.path == "/drain/after-reload" {
            1
        } else {
            0
        };
        assert_eq!(
            observation.revision, expected,
            "unexpected generation for {:?}",
            observation.path
        );
    }

    server.write_config(&config(port, upstream.port, 2));
    server.reload();
    wait_for_version(&server, "rev-2");
    wait_for_no_active(&upstream, Duration::from_secs(5));
    for revision in 0..=2 {
        wait_until(Duration::from_secs(5), "temporary file cleanup", || {
            files_under(&root.join(format!("temp-{revision}/proxy"))).is_empty()
        });
    }
    server.assert_running();
    upstream.stop();
}

fn request_half_closed(port: u16, path: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect half-close client");
    socket2::SockRef::from(&stream)
        .set_recv_buffer_size(64 * 1024)
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    stream.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    response
}

fn request_reset(port: u16, path: &str) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect reset client");
    socket2::SockRef::from(&stream)
        .set_recv_buffer_size(4 * 1024)
        .unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut prefix = [0_u8; 4096];
    let read = stream.read(&mut prefix).expect("read reset prefix");
    assert!(read > 0, "reset client received no response bytes");
    socket2::SockRef::from(&stream)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(stream);
}

fn assert_complete_response(response: &[u8]) {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("half-close response headers")
        + 4;
    assert!(
        response.starts_with(b"HTTP/1.1 200 OK\r\n"),
        "{}",
        String::from_utf8_lossy(&response[..head_end])
    );
    assert_eq!(response.len() - head_end, BODY_SIZE);
}

fn response_body(response: &str) -> &str {
    response.split_once("\r\n\r\n").expect("response headers").1
}

fn wait_for_version(server: &RunningServer, expected: &str) {
    wait_until(Duration::from_secs(5), "configuration reload", || {
        server.get("/version").ends_with(expected)
    });
}

fn wait_for_store(root: &Path, relative: &str) {
    let path = root.join("html").join(relative);
    wait_until(Duration::from_secs(10), "proxy_store publication", || {
        fs::metadata(&path).is_ok_and(|metadata| metadata.len() == BODY_SIZE as u64)
    });
}

fn wait_for_no_active(upstream: &MixedUpstream, timeout: Duration) {
    wait_until(timeout, "upstream connection drain", || {
        upstream.active() == 0
    });
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
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

fn config(port: u16, upstream_port: u16, revision: u64) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            worker_shutdown_timeout 250ms;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 256;
            }}

            http {{
                default_type application/octet-stream;
                access_log off;
                client_body_temp_path temp-{revision}/client-body;
                proxy_temp_path temp-{revision}/proxy;
                proxy_cache_path state/cache levels=1:2 keys_zone=mixed_cache:1m inactive=60m;

                upstream mixed_backend {{
                    server 127.0.0.1:{upstream_port};
                    keepalive 2;
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    root html;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location /drain/ {{
                        proxy_pass http://mixed_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                        proxy_buffer_size 1k;
                        proxy_buffers 4 4k;
                        proxy_busy_buffers_size 8k;
                        proxy_temp_file_write_size 4k;
                        proxy_max_temp_file_size 4m;
                        proxy_read_timeout 10s;
                        proxy_ignore_client_abort on;
                        proxy_cache mixed_cache;
                        proxy_cache_key "$host$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                    }}

                    location /cancel/ {{
                        proxy_pass http://mixed_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                        proxy_buffering off;
                        proxy_buffer_size 1k;
                        proxy_buffers 4 4k;
                        proxy_busy_buffers_size 8k;
                        proxy_temp_file_write_size 4k;
                        proxy_max_temp_file_size 4m;
                        proxy_read_timeout 10s;
                    }}
                }}
            }}
        "#
    )
}
