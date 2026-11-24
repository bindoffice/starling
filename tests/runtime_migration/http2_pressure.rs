use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use tokio::net::TcpStream as TokioTcpStream;

use super::http2_pressure_support::{BODY_SIZE, PressureUpstream};
use super::reload_stress_support::ProcessResources;
use super::support::{RunningServer, TestDirectory, reserve_port};

const ROUNDS: usize = 4;
const STREAMS: usize = 8;
const RESET_STREAMS: usize = 2;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local TCP bind; run with --ignored"]
async fn http2_reload_pressure_keeps_cache_store_and_keepalive_consistent() {
    let port = reserve_port();
    let Some(upstream) = PressureUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("http2-pressure");
    let root = directory.path().to_path_buf();
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));
    let (mut setup_client, setup_driver) = client_for(port).await;
    wait_for_h2_version(&mut setup_client, port, "rev-0", &root).await;
    drop(setup_client);
    setup_driver.abort();
    let baseline = ProcessResources::capture(server.pid());

    let stream = TokioTcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect HTTP/2 pressure client");
    let (mut client, connection) = h2::client::handshake(stream)
        .await
        .expect("HTTP/2 pressure handshake");
    let mut driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut retired_drivers = Vec::new();

    for round in 0..ROUNDS {
        let mut streams = Vec::new();
        for index in 0..STREAMS {
            let mut request_client = client.clone();
            streams.push(tokio::spawn(async move {
                let path = format!("/slow/{round}-{index}");
                let request = http::Request::builder()
                    .method("GET")
                    .uri(format!("http://localhost:{port}{path}"))
                    .body(())
                    .unwrap();
                let (response, mut request_stream) = request_client
                    .send_request(request, true)
                    .expect("send HTTP/2 pressure request");
                let response = response.await.expect("HTTP/2 pressure response");
                assert_eq!(response.status(), http::StatusCode::OK);
                let mut body = response.into_body();
                let first = body
                    .data()
                    .await
                    .expect("HTTP/2 pressure body prefix")
                    .expect("HTTP/2 pressure body prefix");
                assert!(!first.is_empty(), "empty HTTP/2 pressure body prefix");
                let first_len = first.len();
                body.flow_control()
                    .release_capacity(first_len)
                    .expect("release HTTP/2 pressure prefix capacity");
                if index < RESET_STREAMS {
                    request_stream.send_reset(h2::Reason::CANCEL);
                    return;
                }
                let mut received = first_len;
                while let Some(chunk) = body.data().await {
                    let chunk = chunk.expect("HTTP/2 pressure response body");
                    let length = chunk.len();
                    body.flow_control()
                        .release_capacity(length)
                        .expect("release HTTP/2 pressure body capacity");
                    received += length;
                }
                assert_eq!(received, BODY_SIZE, "HTTP/2 pressure response length");
            }));
        }

        upstream.wait_for_requests(round * (STREAMS + 2) + STREAMS, Duration::from_secs(10));
        if round + 1 < ROUNDS {
            server.write_config(&config(port, upstream.port, (round + 1) as u64));
            server.reload();
            // Reload drains old HTTP/2 connections with GOAWAY, matching
            // nginx worker replacement. New requests use a new connection.
            let (next_client, next_driver) =
                connect_and_wait_for_h2_version(port, &format!("rev-{}", round + 1), &root).await;
            let previous_client = std::mem::replace(&mut client, next_client);
            let previous_driver = std::mem::replace(&mut driver, next_driver);
            drop(previous_client);
            retired_drivers.push(previous_driver);
        }
        for stream in streams {
            if let Err(error) = stream.await {
                panic!(
                    "HTTP/2 pressure stream panicked: {error}\n{}",
                    fs::read_to_string(root.join("logs/error.log")).unwrap_or_default()
                );
            }
        }
        for retired in retired_drivers.drain(..) {
            retired.abort();
        }

        for index in 0..STREAMS {
            let target = format!("/slow/{round}-{index}");
            wait_for_store(&root, &root.join(format!("state/html{target}")), BODY_SIZE);
            assert_cache_hit(&mut client, port, &target).await;
        }

        let requests_before = upstream.stats.requests();
        probe(&mut client, port, &format!("/probe/{round}-a")).await;
        probe(&mut client, port, &format!("/probe/{round}-b")).await;
        assert_eq!(
            upstream.stats.requests(),
            requests_before + 2,
            "uncached probes did not reach the upstream"
        );
    }

    assert!(
        upstream.stats.reused() > 0,
        "named upstream keepalive did not reuse any connection"
    );
    assert!(
        upstream.stats.mixed_revisions().is_empty(),
        "one upstream connection mixed reload generations: {:?}",
        upstream.stats.mixed_revisions()
    );
    assert!(
        upstream.stats.revisions().len() >= ROUNDS,
        "too few upstream generations observed: {:?}",
        upstream.stats.revisions()
    );
    wait_for_temp_cleanup(&root);
    let final_resources = ProcessResources::capture(server.pid());
    if let (Some(baseline), Some(final_resources)) = (baseline, final_resources) {
        assert!(
            final_resources.fds <= baseline.fds + 8,
            "FDs grew under HTTP/2 pressure: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.tasks <= baseline.tasks + 32,
            "tasks did not settle under HTTP/2 pressure: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "RSS grew under HTTP/2 pressure: baseline={baseline:?} final={final_resources:?}"
        );
    }
    server.assert_running();

    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    upstream.wait_for_no_active(Duration::from_secs(5));
    upstream.stop();
    drop(client);
    driver.abort();
}

async fn client_for(
    port: u16,
) -> (
    h2::client::SendRequest<bytes::Bytes>,
    tokio::task::JoinHandle<()>,
) {
    let stream = TokioTcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect HTTP/2 setup client");
    let (client, connection) = h2::client::handshake(stream)
        .await
        .expect("HTTP/2 setup handshake");
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    (client, driver)
}

async fn h2_get(
    client: &mut h2::client::SendRequest<bytes::Bytes>,
    port: u16,
    target: &str,
) -> (http::StatusCode, http::HeaderMap, Vec<u8>) {
    h2_get_result(client, port, target)
        .await
        .expect("HTTP/2 control request")
}

async fn h2_get_result(
    client: &mut h2::client::SendRequest<bytes::Bytes>,
    port: u16,
    target: &str,
) -> Result<(http::StatusCode, http::HeaderMap, Vec<u8>), h2::Error> {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("http://localhost:{port}{target}"))
        .body(())
        .unwrap();
    let (response, _) = client.send_request(request, true)?;
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .expect("HTTP/2 control response timeout")?;
    let status = response.status();
    let headers = response.headers().clone();
    let mut body = response.into_body();
    let mut received = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk?;
        body.flow_control().release_capacity(chunk.len())?;
        received.extend_from_slice(&chunk);
    }
    Ok((status, headers, received))
}

async fn wait_for_h2_version(
    client: &mut h2::client::SendRequest<bytes::Bytes>,
    port: u16,
    expected: &str,
    root: &Path,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, _, body) = h2_get(client, port, "/version").await;
        if status == http::StatusCode::OK && body == expected.as_bytes() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "HTTP/2 reload did not publish {expected}: status={status} body={:?}\n{}",
            String::from_utf8_lossy(&body),
            fs::read_to_string(root.join("logs/error.log")).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn connect_and_wait_for_h2_version(
    port: u16,
    expected: &str,
    root: &Path,
) -> (
    h2::client::SendRequest<bytes::Bytes>,
    tokio::task::JoinHandle<()>,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (mut client, driver) = client_for(port).await;
        match h2_get_result(&mut client, port, "/version").await {
            Ok((status, _, body))
                if status == http::StatusCode::OK && body == expected.as_bytes() =>
            {
                return (client, driver);
            }
            Ok((status, _, body)) => {
                assert!(
                    Instant::now() < deadline,
                    "HTTP/2 reload did not publish {expected}: status={status} body={:?}\n{}",
                    String::from_utf8_lossy(&body),
                    fs::read_to_string(root.join("logs/error.log")).unwrap_or_default()
                );
            }
            Err(_) => {
                assert!(
                    Instant::now() < deadline,
                    "HTTP/2 reload did not publish {expected}\n{}",
                    fs::read_to_string(root.join("logs/error.log")).unwrap_or_default()
                );
            }
        }
        drop(client);
        driver.abort();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn assert_cache_hit(
    client: &mut h2::client::SendRequest<bytes::Bytes>,
    port: u16,
    target: &str,
) {
    let (status, headers, body) = h2_get(client, port, target).await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body.len(), BODY_SIZE, "cache hit body length for {target}");
    assert!(
        headers
            .get("x-cache-status")
            .is_some_and(|value| value == "HIT"),
        "cache did not publish {target}: {headers:?}"
    );
}

async fn probe(client: &mut h2::client::SendRequest<bytes::Bytes>, port: u16, target: &str) {
    let (status, _, body) = h2_get(client, port, target).await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(
        body.starts_with(b"probe rev "),
        "unexpected probe body: {:?}",
        String::from_utf8_lossy(&body)
    );
}

fn wait_for_store(root: &Path, path: &Path, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !fs::metadata(path).is_ok_and(|metadata| metadata.len() == expected as u64) {
        let diagnostics = format!(
            "error log:\n{}\nstderr:\n{}",
            fs::read_to_string(root.join("logs/error.log")).unwrap_or_default(),
            fs::read_to_string(root.join("logs/stderr.log")).unwrap_or_default()
        );
        assert!(
            Instant::now() < deadline,
            "timed out waiting for HTTP/2 store publication at {}\nfiles={:?}\n\
             cache={:?}\n{}",
            path.display(),
            files_under(path.parent().unwrap_or(root)),
            files_under(&root.join("state/cache")),
            diagnostics
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_temp_cleanup(root: &Path) {
    wait_until(Duration::from_secs(10), "HTTP/2 temp cleanup", || {
        temporary_files(root).is_empty()
    });
}

fn temporary_files(root: &Path) -> Vec<PathBuf> {
    files_under(root)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".starling-"))
        })
        .collect()
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
                worker_connections 512;
            }}

            http {{
                default_type application/octet-stream;
                access_log off;
                client_body_temp_path temp-{revision}/client-body;
                proxy_temp_path temp-{revision}/proxy;
                proxy_cache_path state/cache levels=1:2
                                 keys_zone=http2_pressure:1m inactive=60m;

                upstream http2_backend {{
                    server 127.0.0.1:{upstream_port};
                    keepalive 4;
                }}

                server {{
                    listen 127.0.0.1:{port} http2;
                    server_name localhost;
                    root state/html;

                    location = /version {{
                        return 200 "rev-{revision}";
                    }}

                    location /slow/ {{
                        proxy_pass http://http2_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                        proxy_buffer_size 1k;
                        proxy_buffers 4 1k;
                        proxy_busy_buffers_size 2k;
                        proxy_temp_file_write_size 1k;
                        proxy_max_temp_file_size 4m;
                        proxy_read_timeout 5s;
                        proxy_ignore_client_abort on;
                        proxy_cache http2_pressure;
                        proxy_cache_key "$request_uri";
                        proxy_cache_valid 200 1m;
                        proxy_store on;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}

                    location /probe/ {{
                        proxy_pass http://http2_backend;
                        proxy_http_version 1.1;
                        proxy_set_header Host $host;
                        proxy_set_header Connection "";
                        proxy_set_header X-Config-Revision "{revision}";
                        proxy_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}
