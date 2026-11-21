use std::env;
use std::fs;
use std::io::{self, Read};
use std::net::TcpStream;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::reload_stress_support::{ProcessResources, StressUpstream, config};
use super::support::{Http1Connection, RunningServer, TestDirectory, assert_status, reserve_port};

const RELOADS: u64 = 24;
const TRAFFIC_CLIENTS: usize = 8;
const IDLE_CLIENTS: usize = 8;
const SOAK_SECONDS_ENV: &str = "STARLING_RELOAD_SOAK_SECONDS";

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn repeated_reload_keepalive_traffic_and_sigquit_leave_no_runtime_leaks() {
    let port = reserve_port();
    let Some(upstream) = StressUpstream::start() else {
        return;
    };
    let directory = TestDirectory::new("reload-stress");
    let mut server = RunningServer::start(directory, port, &config(port, upstream.port, 0));

    let mut control = Http1Connection::connect(port);
    wait_for_version(&mut control, "rev-0");
    let baseline = ProcessResources::capture(server.pid());

    let stop = Arc::new(AtomicBool::new(false));
    let mut traffic = Vec::new();
    for _ in 0..TRAFFIC_CLIENTS {
        let stop = stop.clone();
        traffic.push(thread::spawn(move || {
            let mut connection = Http1Connection::connect(port);
            let mut completed = 0_usize;
            while !stop.load(Ordering::Relaxed) {
                let response = connection.get("/proxied");
                assert_status(&response, 200);
                assert!(
                    response.contains("proxied"),
                    "unexpected proxied response:\n{response}"
                );
                completed += 1;
            }
            completed
        }));
    }

    let soak = env::var(SOAK_SECONDS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::ZERO);
    let soak_started = Instant::now();
    let mut resource_samples = Vec::new();
    let mut revision = 0_u64;
    loop {
        let short_run_complete = soak.is_zero() && revision >= RELOADS;
        let soak_complete = !soak.is_zero() && soak_started.elapsed() >= soak;
        if short_run_complete || soak_complete {
            break;
        }
        revision += 1;
        server.write_config(&config(port, upstream.port, revision));
        server.reload();
        let version = format!("rev-{revision}");
        wait_for_version(&mut control, &version);

        // A proxied request after each reload forces a new generation to connect.
        let response = control.get("/proxied");
        assert_status(&response, 200);
        assert!(
            response.contains("proxied"),
            "proxied request after {version} failed:\n{response}"
        );
        if let Some(resources) = ProcessResources::capture(server.pid()) {
            if revision.is_multiple_of(8) {
                eprintln!(
                    "reload soak t={:.1}s revision={revision} {resources:?}",
                    soak_started.elapsed().as_secs_f64()
                );
            }
            resource_samples.push(resources);
        }
    }

    stop.store(true, Ordering::Relaxed);
    let completed = traffic
        .into_iter()
        .map(|client| client.join().expect("keepalive traffic client panicked"))
        .sum::<usize>();
    assert!(
        completed >= TRAFFIC_CLIENTS,
        "keepalive traffic did not run during reloads: {completed} requests"
    );

    // No request races this final clear, so every pooled upstream connection must close.
    server.write_config(&config(port, upstream.port, revision));
    server.reload();
    wait_for_version(&mut control, &format!("rev-{revision}"));
    upstream.wait_for_no_active(Duration::from_secs(5));

    assert!(
        upstream.stats.opened() >= revision as usize,
        "each reload generation should establish an upstream connection: {:?}",
        upstream.stats.summary()
    );
    assert_eq!(
        upstream.stats.mixed_revisions(),
        Vec::<String>::new(),
        "an upstream keepalive connection served more than one config generation"
    );
    assert!(
        upstream.stats.revisions().len() >= 8,
        "reload stress did not exercise distinct proxy configurations: {:?}",
        upstream.stats.summary()
    );

    thread::sleep(Duration::from_millis(200));
    let final_resources = ProcessResources::capture(server.pid());
    if let (Some(baseline), Some(final_resources)) = (baseline, final_resources) {
        let peak_rss_kib = resource_samples
            .iter()
            .map(|sample| sample.rss_kib)
            .max()
            .unwrap_or(final_resources.rss_kib);
        assert!(
            final_resources.fds <= baseline.fds + 4,
            "file descriptors grew across reloads: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "RSS grew across reloads: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_rss_kib <= baseline.rss_kib + 96 * 1024,
            "RSS peak grew too far during the soak: baseline={baseline:?} peak={peak_rss_kib} KiB"
        );
    }
    assert!(
        files_under(&server.directory_path().join("temp")).is_empty(),
        "reload stress left temporary files behind"
    );

    let mut idle = Vec::new();
    for _ in 0..IDLE_CLIENTS {
        let mut connection = Http1Connection::connect(port);
        wait_for_version(&mut connection, &format!("rev-{revision}"));
        idle.push(connection.into_stream());
    }
    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    for mut connection in idle {
        wait_for_eof(&mut connection);
    }
    wait_for_eof(&mut control.into_stream());
    upstream.stop();
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

fn wait_for_eof(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut byte = [0_u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
                ) =>
            {
                return;
            }
            Err(error) => panic!("idle keepalive client was not closed: {error}"),
        }
    }
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
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
