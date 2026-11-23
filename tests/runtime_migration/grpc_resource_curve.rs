use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use tokio::net::TcpListener;

use super::reload_stress_support::ProcessResources;
use super::support::{RunningServer, TestDirectory, reserve_port};

const REQUESTS: usize = 24;
const REQUEST_BODY_SIZE: usize = 48 * 1024;
const RESPONSE_BODY_SIZE: usize = 64 * 1024;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local TCP bind; run with --ignored"]
async fn grpc_runtime_resource_curve_stays_bounded() {
    let backend_listener = match TcpListener::bind(("127.0.0.1", 0)).await {
        Ok(listener) => listener,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping gRPC runtime resource curve: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind gRPC resource backend: {error}"),
    };
    let backend_port = backend_listener.local_addr().unwrap().port();
    let served = Arc::new(AtomicUsize::new(0));
    let backend_errors = Arc::new(Mutex::new(Vec::new()));
    let backend = tokio::spawn(run_backend(
        backend_listener,
        served.clone(),
        backend_errors.clone(),
    ));

    let port = reserve_port();
    let directory = TestDirectory::new("grpc-resource-curve");
    let mut server = RunningServer::start(directory, port, &config(port, backend_port));
    let baseline = ProcessResources::capture(server.pid());
    let client_stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect gRPC resource client");
    let (mut client, connection) = h2::client::handshake(client_stream)
        .await
        .expect("gRPC resource client handshake");
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });

    let mut samples = Vec::new();
    for index in 0..REQUESTS {
        grpc_request(&mut client, port, index)
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "gRPC request {index} failed: {error}\nbackend errors: {:?}\n{}",
                    backend_errors.lock().unwrap(),
                    fs::read_to_string(server.directory_path().join("logs/error.log"))
                        .unwrap_or_default()
                )
            });
        if index % 4 == 0
            && let Some(resources) = ProcessResources::capture(server.pid())
        {
            eprintln!(
                "gRPC resource sample request={index} fds={} tasks={} rss_kib={}",
                resources.fds, resources.tasks, resources.rss_kib
            );
            samples.push(resources);
        }
    }

    wait_until(Duration::from_secs(5), "gRPC request drain", || {
        served.load(Ordering::Relaxed) == REQUESTS
    });
    assert!(
        backend_errors.lock().unwrap().is_empty(),
        "gRPC backend failures: {:?}",
        backend_errors.lock().unwrap()
    );
    wait_until(Duration::from_secs(5), "gRPC temp cleanup", || {
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
            "gRPC resource curve leaked FDs: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_tasks <= baseline.tasks + 32,
            "gRPC resource curve created too many tasks: baseline={baseline:?} peak={peak_tasks}"
        );
        assert!(
            final_resources.tasks <= baseline.tasks + 32,
            "gRPC tasks did not settle: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            final_resources.rss_kib <= baseline.rss_kib + 64 * 1024,
            "gRPC resource curve leaked RSS: baseline={baseline:?} final={final_resources:?}"
        );
        assert!(
            peak_rss_kib <= baseline.rss_kib + 96 * 1024,
            "gRPC resource curve peaked too high: baseline={baseline:?} peak={peak_rss_kib} KiB"
        );
    }
    server.assert_running();
    server.shutdown_gracefully();
    server.wait_for_exit(Duration::from_secs(5));
    drop(client);
    driver.abort();
    let backend_result = tokio::time::timeout(Duration::from_secs(5), backend)
        .await
        .expect("gRPC backend did not observe connection shutdown");
    backend_result.expect("gRPC backend panicked");
}

fn config(port: u16, backend_port: u16) -> String {
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
                default_type application/grpc;
                access_log off;
                client_body_temp_path temp/client-body;
                proxy_temp_path temp/proxy;

                upstream grpc_resource_backend {{
                    server 127.0.0.1:{backend_port};
                    keepalive 1;
                }}

                server {{
                    listen 127.0.0.1:{port} http2;
                    server_name localhost;

                    location / {{
                        grpc_pass grpc_resource_backend;
                        grpc_request_buffering off;
                        grpc_read_timeout 5s;
                        grpc_send_timeout 5s;
                    }}
                }}
            }}
        "#
    )
}

async fn grpc_request(
    client: &mut h2::client::SendRequest<Bytes>,
    port: u16,
    index: usize,
) -> Result<(), String> {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "http://localhost:{port}/pkg.Resource/Echo?request={index}"
        ))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("content-length", REQUEST_BODY_SIZE.to_string())
        .body(())
        .map_err(|error| error.to_string())?;
    let (response, mut request_body) = client
        .send_request(request, false)
        .map_err(|error| error.to_string())?;
    let payload = format!("request-{index}-");
    let mut body = payload.into_bytes();
    body.resize(REQUEST_BODY_SIZE, b'q');
    request_body
        .send_data(Bytes::from(body), true)
        .map_err(|error| error.to_string())?;

    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .map_err(|_| "response timeout".to_string())?
        .map_err(|error| error.to_string())?;
    if response.status() != http::StatusCode::OK {
        return Err(format!("unexpected status {}", response.status()));
    }
    if response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| !value.starts_with("application/grpc"))
    {
        return Err("missing gRPC content type".to_string());
    }

    let mut response_body = response.into_body();
    let mut received = 0;
    while let Some(chunk) = response_body.data().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        received += chunk.len();
        response_body
            .flow_control()
            .release_capacity(chunk.len())
            .map_err(|error| error.to_string())?;
    }
    if received != RESPONSE_BODY_SIZE {
        return Err(format!(
            "expected {RESPONSE_BODY_SIZE} response bytes, got {received}"
        ));
    }
    let trailers = response_body
        .trailers()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "missing gRPC trailers".to_string())?;
    if trailers
        .get("grpc-status")
        .and_then(|value| value.to_str().ok())
        != Some("0")
    {
        return Err(format!("unexpected gRPC trailers: {trailers:?}"));
    }
    Ok(())
}

async fn run_backend(
    listener: TcpListener,
    served: Arc<AtomicUsize>,
    errors: Arc<Mutex<Vec<String>>>,
) {
    let (stream, _) = match listener.accept().await {
        Ok(connection) => connection,
        Err(error) => {
            errors.lock().unwrap().push(format!("accept: {error}"));
            return;
        }
    };
    eprintln!("gRPC resource backend accepted a connection");
    let mut connection = match h2::server::handshake(stream).await {
        Ok(connection) => connection,
        Err(error) => {
            errors.lock().unwrap().push(format!("handshake: {error}"));
            return;
        }
    };
    let mut streams = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            accepted = connection.accept() => {
                let Some(accepted) = accepted else {
                    break;
                };
                let (request, respond) = match accepted {
                    Ok(request) => request,
                    Err(error) => {
                        errors.lock().unwrap().push(format!("stream: {error}"));
                        break;
                    }
                };
                streams.spawn(handle_stream(
                    request,
                    respond,
                    served.clone(),
                    errors.clone(),
                ));
            }
            joined = streams.join_next(), if !streams.is_empty() => {
                if let Some(Err(error)) = joined {
                    errors
                        .lock()
                        .unwrap()
                        .push(format!("stream task: {error}"));
                }
            }
        }
    }
    while let Some(joined) = streams.join_next().await {
        if let Err(error) = joined {
            errors.lock().unwrap().push(format!("stream task: {error}"));
        }
    }
}

async fn handle_stream(
    request: http::Request<h2::RecvStream>,
    mut respond: h2::server::SendResponse<Bytes>,
    served: Arc<AtomicUsize>,
    errors: Arc<Mutex<Vec<String>>>,
) {
    let path = request.uri().path().to_string();
    let mut request_body = request.into_body();
    let response = match http::Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
    {
        Ok(response) => response,
        Err(error) => {
            errors.lock().unwrap().push(format!("response: {error}"));
            return;
        }
    };
    let mut response_body = match respond.send_response(response, false) {
        Ok(body) => body,
        Err(error) => {
            errors
                .lock()
                .unwrap()
                .push(format!("{path}: headers: {error}"));
            return;
        }
    };
    let response_payload = vec![b'x'; RESPONSE_BODY_SIZE];
    if let Err(error) = response_body.send_data(Bytes::from(response_payload), false) {
        errors
            .lock()
            .unwrap()
            .push(format!("{path}: data: {error}"));
        return;
    }
    let mut trailers = http::HeaderMap::new();
    trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
    trailers.insert(
        "grpc-message",
        http::HeaderValue::from_static("resource curve"),
    );
    if let Err(error) = response_body.send_trailers(trailers) {
        errors
            .lock()
            .unwrap()
            .push(format!("{path}: trailers: {error}"));
    }
    let mut received = Vec::new();
    let body_result = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(chunk) = request_body.data().await {
            match chunk {
                Ok(chunk) => received.extend_from_slice(&chunk),
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(())
    })
    .await;
    match body_result {
        Ok(Ok(())) if received.len() == REQUEST_BODY_SIZE => {}
        Ok(Ok(())) => errors.lock().unwrap().push(format!(
            "{path}: expected {REQUEST_BODY_SIZE} request bytes, got {}",
            received.len()
        )),
        Ok(Err(error)) => errors
            .lock()
            .unwrap()
            .push(format!("{path}: body: {error}")),
        Err(_) => errors
            .lock()
            .unwrap()
            .push(format!("{path}: request body timed out")),
    }
    served.fetch_add(1, Ordering::Relaxed);
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
        } else {
            files.push(path);
        }
    }
    files
}

fn wait_until(timeout: Duration, label: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
