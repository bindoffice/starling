use std::fs;
use std::io;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::fastcgi_support::{
    configure_stream as configure_fastcgi, read_fastcgi_request, write_fastcgi_response,
};
use super::gateway_support::{
    Gateway, configure_stream, read_gateway_params, write_gateway_response,
};
use super::reload_stress_support::ProcessResources;
use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

const REQUESTS_PER_GATEWAY: usize = 8;
const GATEWAY_BODY_SIZE: usize = 32 * 1024;

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn fastcgi_uwsgi_and_scgi_runtime_curves_stay_bounded() {
    let Some(fastcgi) = FakeGateway::start(GatewayKind::Fastcgi) else {
        return;
    };
    let Some(uwsgi) = FakeGateway::start(GatewayKind::Uwsgi) else {
        return;
    };
    let Some(scgi) = FakeGateway::start(GatewayKind::Scgi) else {
        return;
    };
    let port = reserve_port();
    let directory = TestDirectory::new("gateway-resource-curve");
    directory.write("temp/client-body/.keep", "");
    directory.write("temp/proxy/.keep", "");
    let mut server = RunningServer::start(
        directory,
        port,
        &config(port, fastcgi.port, uwsgi.port, scgi.port),
    );
    let baseline = ProcessResources::capture(server.pid());
    let mut samples = Vec::new();

    for gateway in [&fastcgi, &uwsgi, &scgi] {
        for index in 0..REQUESTS_PER_GATEWAY {
            let response = server.get(&gateway.target(index));
            assert_status(&response, 200);
            assert!(
                response.ends_with(&"g".repeat(GATEWAY_BODY_SIZE)),
                "{} response body was incomplete at request {index}",
                gateway.label()
            );
        }
        capture_sample(server.pid(), gateway.label(), &mut samples);
    }

    for gateway in [&fastcgi, &uwsgi, &scgi] {
        wait_until(
            Duration::from_secs(5),
            &format!("{} connection drain", gateway.label()),
            || gateway.active.load(Ordering::Relaxed) == 0,
        );
        assert_eq!(
            gateway.requests.load(Ordering::Relaxed),
            REQUESTS_PER_GATEWAY,
            "{} request count",
            gateway.label()
        );
    }
    wait_until(Duration::from_secs(5), "gateway temp cleanup", || {
        files_under(&server.directory_path().join("temp")).is_empty()
    });
    let final_resources = ProcessResources::capture(server.pid());
    if let (Some(baseline), Some(final_resources)) = (baseline, final_resources) {
        let peak_rss_kib = samples
            .iter()
            .map(|sample| sample.rss_kib)
            .chain(std::iter::once(final_resources.rss_kib))
            .max()
            .unwrap_or(final_resources.rss_kib);
        let peak_tasks = samples
            .iter()
            .map(|sample| sample.tasks)
            .chain(std::iter::once(final_resources.tasks))
            .max()
            .unwrap_or(final_resources.tasks);
        assert!(
            final_resources.fds <= baseline.fds + 8,
            "gateway curves leaked FDs: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_tasks <= baseline.tasks + 32,
            "gateway curves created too many tasks: baseline={baseline:?} peak={peak_tasks}"
        );
        assert!(
            final_resources.tasks <= baseline.tasks + 32,
            "gateway tasks did not settle: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "gateway curves leaked RSS: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_rss_kib <= baseline.rss_kib + 96 * 1024,
            "gateway curves peaked too high: baseline={baseline:?} peak={peak_rss_kib} KiB"
        );
    }
    server.assert_running();
    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    fastcgi.stop();
    uwsgi.stop();
    scgi.stop();
}

fn capture_sample(pid: u32, label: &str, samples: &mut Vec<ProcessResources>) {
    if let Some(resources) = ProcessResources::capture(pid) {
        eprintln!(
            "gateway resource sample gateway={label} fds={} tasks={} rss_kib={}",
            resources.fds, resources.tasks, resources.rss_kib
        );
        samples.push(resources);
    }
}

fn config(port: u16, fastcgi_port: u16, uwsgi_port: u16, scgi_port: u16) -> String {
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
                default_type text/plain;
                access_log off;
                client_body_temp_path temp/client-body;
                proxy_temp_path temp/proxy;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /fastcgi/ {{
                        fastcgi_pass 127.0.0.1:{fastcgi_port};
                        fastcgi_param REQUEST_METHOD $request_method;
                        fastcgi_param REQUEST_URI $request_uri;
                        fastcgi_read_timeout 5s;
                    }}

                    location /uwsgi/ {{
                        uwsgi_pass 127.0.0.1:{uwsgi_port};
                        uwsgi_param REQUEST_METHOD $request_method;
                        uwsgi_param REQUEST_URI $request_uri;
                        uwsgi_read_timeout 5s;
                    }}

                    location /scgi/ {{
                        scgi_pass 127.0.0.1:{scgi_port};
                        scgi_param REQUEST_METHOD $request_method;
                        scgi_param REQUEST_URI $request_uri;
                        scgi_read_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

#[derive(Clone, Copy)]
enum GatewayKind {
    Fastcgi,
    Uwsgi,
    Scgi,
}

impl GatewayKind {
    fn label(self) -> &'static str {
        match self {
            Self::Fastcgi => "FastCGI",
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }

    fn prefix(self) -> &'static str {
        match self {
            Self::Fastcgi => "/fastcgi/",
            Self::Uwsgi => "/uwsgi/",
            Self::Scgi => "/scgi/",
        }
    }
}

struct FakeGateway {
    port: u16,
    kind: GatewayKind,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    accept: Option<thread::JoinHandle<()>>,
}

impl FakeGateway {
    fn start(kind: GatewayKind) -> Option<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                eprintln!("skipping gateway resource curve: TCP binding is not permitted");
                return None;
            }
            Err(error) => panic!("bind gateway resource upstream: {error}"),
        };
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let accept = thread::spawn({
            let requests = requests.clone();
            let active = active.clone();
            let stop = stop.clone();
            move || accept_loop(listener, kind, requests, active, stop)
        });
        Some(Self {
            port,
            kind,
            requests,
            active,
            stop,
            accept: Some(accept),
        })
    }

    fn label(&self) -> &'static str {
        self.kind.label()
    }

    fn target(&self, index: usize) -> String {
        format!("{}{}", self.kind.prefix(), index)
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(accept) = self.accept.take() {
            accept.join().unwrap();
        }
    }
}

fn accept_loop(
    listener: TcpListener,
    kind: GatewayKind,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let requests = requests.clone();
                let active = active.clone();
                thread::spawn(move || serve_connection(kind, stream, requests, active));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("gateway resource upstream accept failed: {error}"),
        }
    }
}

fn serve_connection(
    kind: GatewayKind,
    stream: std::net::TcpStream,
    requests: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
) -> io::Result<()> {
    active.fetch_add(1, Ordering::Relaxed);
    let mut stream = stream;
    let result = match kind {
        GatewayKind::Fastcgi => {
            configure_fastcgi(&stream)?;
            let (request_id, _) = read_fastcgi_request(&mut stream)?;
            write_fastcgi_response(
                &mut stream,
                request_id,
                "Cache-Control: max-age=60",
                &"g".repeat(GATEWAY_BODY_SIZE),
            )
        }
        GatewayKind::Uwsgi => {
            configure_stream(&stream)?;
            let _ = read_gateway_params(Gateway::Uwsgi, &mut stream)?;
            write_gateway_response(
                &mut stream,
                "Cache-Control: max-age=60",
                &"g".repeat(GATEWAY_BODY_SIZE),
            )
        }
        GatewayKind::Scgi => {
            configure_stream(&stream)?;
            let _ = read_gateway_params(Gateway::Scgi, &mut stream)?;
            write_gateway_response(
                &mut stream,
                "Cache-Control: max-age=60",
                &"g".repeat(GATEWAY_BODY_SIZE),
            )
        }
    };
    if result.is_ok() {
        requests.fetch_add(1, Ordering::Relaxed);
    }
    active.fetch_sub(1, Ordering::Relaxed);
    result
}

fn files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_under(&path));
        } else if path.file_name().is_none_or(|name| name != ".keep") {
            files.push(path);
        }
    }
    files
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        thread::sleep(Duration::from_millis(20));
    }
}
