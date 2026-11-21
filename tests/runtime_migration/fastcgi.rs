use std::collections::HashMap;
use std::io::{self, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::fastcgi_support::{configure_stream, read_fastcgi_request, write_fastcgi_response};
use super::support::{RunningServer, TestDirectory, assert_status, request, reserve_port};

type FastCgiParams = HashMap<String, String>;
type BackgroundUpdateResult = (FastCgiParams, FastCgiParams);

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn runs_fastcgi_and_serves_the_second_request_from_cache() {
    let port = reserve_port();
    let Some(upstream) = fake_fastcgi() else {
        return;
    };
    let directory = TestDirectory::new("fastcgi-cache");
    let config = fastcgi_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let first = server.get("/app/index.php?mode=runtime");
    assert_status(&first, 200);
    assert!(
        first.to_ascii_lowercase().contains("x-cache-status: miss"),
        "first FastCGI response was not a cache miss:\n{first}"
    );
    assert!(
        first.contains("fastcgi-runtime-body"),
        "FastCGI response body missing:\n{first}"
    );

    let params = upstream.captured();
    assert_eq!(
        params.get("REQUEST_METHOD").map(String::as_str),
        Some("GET")
    );
    assert_eq!(
        params.get("QUERY_STRING").map(String::as_str),
        Some("mode=runtime")
    );
    assert_eq!(
        params.get("SCRIPT_FILENAME").map(String::as_str),
        Some("/srv/app/index.php")
    );

    let second = server.get("/app/index.php?mode=runtime");
    assert_status(&second, 200);
    assert!(
        second.to_ascii_lowercase().contains("x-cache-status: hit"),
        "second FastCGI response was not a cache hit:\n{second}"
    );
    assert!(
        second.contains("fastcgi-runtime-body"),
        "cached FastCGI response body missing:\n{second}"
    );
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn refreshes_expired_fastcgi_cache_in_background_while_serving_stale() {
    let port = reserve_port();
    let Some(upstream) = fake_fastcgi_background_update() else {
        return;
    };
    let directory = TestDirectory::new("fastcgi-cache-background-update");
    let config = fastcgi_background_update_config(port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let first = server.get("/app/index.php?mode=background");
    assert_status(&first, 200);
    assert_cache_status(&first, "miss");
    assert!(
        first.ends_with("old"),
        "unexpected first response:\n{first}"
    );

    thread::sleep(Duration::from_millis(1100));

    let (stale_sender, stale_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = stale_sender.send(request(port, "/app/index.php?mode=background"));
    });
    upstream
        .update_started
        .recv_timeout(Duration::from_secs(5))
        .expect("expired FastCGI cache entry did not start a background update");
    let stale = stale_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("stale FastCGI response waited for the background update");
    assert_status(&stale, 200);
    assert_cache_status(&stale, "stale");
    assert!(
        stale.ends_with("old"),
        "unexpected stale response:\n{stale}"
    );

    let updating = server.get("/app/index.php?mode=background");
    assert_status(&updating, 200);
    assert_cache_status(&updating, "updating");
    assert!(
        updating.ends_with("old"),
        "unexpected updating response:\n{updating}"
    );

    upstream
        .release_update
        .send(())
        .expect("release FastCGI background update");
    let (first_params, second_params) = upstream
        .finished
        .recv_timeout(Duration::from_secs(5))
        .expect("FastCGI background update did not finish")
        .expect("FastCGI background update failed");
    for params in [&first_params, &second_params] {
        assert_eq!(
            params.get("REQUEST_URI").map(String::as_str),
            Some("/app/index.php?mode=background")
        );
    }

    let mut last = String::new();
    for _ in 0..100 {
        thread::sleep(Duration::from_millis(20));
        last = server.get("/app/index.php?mode=background");
        if last.to_ascii_lowercase().contains("x-cache-status: hit") {
            break;
        }
    }
    assert_status(&last, 200);
    assert_cache_status(&last, "hit");
    assert!(
        last.ends_with("new"),
        "FastCGI background update did not replace the cached body:\n{last}"
    );
    server.assert_running();
}

fn fastcgi_config(port: u16, upstream_port: u16) -> String {
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
                fastcgi_cache_path cache levels=1:2
                                   keys_zone=fastcgi_runtime:1m
                                   inactive=60m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /app/ {{
                        fastcgi_pass 127.0.0.1:{upstream_port};
                        fastcgi_param REQUEST_METHOD $request_method;
                        fastcgi_param QUERY_STRING $query_string;
                        fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                        fastcgi_cache fastcgi_runtime;
                        fastcgi_cache_key "$request_method$host$request_uri";
                        fastcgi_cache_valid 200 1m;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}

fn fastcgi_background_update_config(port: u16, upstream_port: u16) -> String {
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
                fastcgi_cache_path cache levels=1:2
                                   keys_zone=fastcgi_background:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /app/ {{
                        fastcgi_pass 127.0.0.1:{upstream_port};
                        fastcgi_param REQUEST_METHOD $request_method;
                        fastcgi_param REQUEST_URI $request_uri;
                        fastcgi_param QUERY_STRING $query_string;
                        fastcgi_param SCRIPT_FILENAME /srv$fastcgi_script_name;
                        fastcgi_cache fastcgi_background;
                        fastcgi_cache_key "$request_method$host$request_uri";
                        fastcgi_cache_valid 200 1s;
                        fastcgi_cache_use_stale updating;
                        fastcgi_cache_background_update on;
                        fastcgi_cache_lock on;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}

fn assert_cache_status(response: &str, expected: &str) {
    assert!(
        response
            .to_ascii_lowercase()
            .contains(&format!("x-cache-status: {expected}\r\n")),
        "expected cache status {expected}:\n{response}"
    );
}

struct FakeFastCgi {
    port: u16,
    params: Receiver<Result<FastCgiParams, String>>,
}

impl FakeFastCgi {
    fn captured(self) -> FastCgiParams {
        self.params
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("FastCGI request capture timed out")
            .unwrap_or_else(|error| panic!("fake FastCGI failed: {error}"))
    }
}

struct FakeFastCgiBackgroundUpdate {
    port: u16,
    update_started: Receiver<()>,
    release_update: Sender<()>,
    finished: Receiver<Result<BackgroundUpdateResult, String>>,
}

fn fake_fastcgi() -> Option<FakeFastCgi> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping runtime FastCGI test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind fake FastCGI upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (sender, params) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_fastcgi(listener);
        let _ = sender.send(result.map_err(|error| error.to_string()));
    });
    Some(FakeFastCgi { port, params })
}

fn fake_fastcgi_background_update() -> Option<FakeFastCgiBackgroundUpdate> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping runtime FastCGI background update test: TCP binding is not permitted"
            );
            return None;
        }
        Err(error) => panic!("bind fake FastCGI upstream: {error}"),
    };
    let port = listener.local_addr().unwrap().port();
    let (update_started, update_started_rx) = mpsc::channel();
    let (release_update, release_update_rx) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_fastcgi_background_update(listener, update_started, release_update_rx);
        let _ = finished.send(result.map_err(|error| error.to_string()));
    });
    Some(FakeFastCgiBackgroundUpdate {
        port,
        update_started: update_started_rx,
        release_update,
        finished: finished_rx,
    })
}

fn serve_fastcgi(listener: TcpListener) -> io::Result<FastCgiParams> {
    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let (request_id, params) = read_fastcgi_request(&mut stream)?;
    write_fastcgi_response(
        &mut stream,
        request_id,
        "Cache-Control: max-age=60",
        "fastcgi-runtime-body",
    )?;
    stream.flush()?;
    Ok(params)
}

fn serve_fastcgi_background_update(
    listener: TcpListener,
    update_started: Sender<()>,
    release_update: Receiver<()>,
) -> io::Result<BackgroundUpdateResult> {
    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let (request_id, first_params) = read_fastcgi_request(&mut stream)?;
    write_fastcgi_response(&mut stream, request_id, "", "old")?;
    stream.flush()?;
    drop(stream);

    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let (request_id, second_params) = read_fastcgi_request(&mut stream)?;
    update_started
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "FastCGI test exited"))?;
    release_update
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "FastCGI background update was not released",
            )
        })?;
    write_fastcgi_response(&mut stream, request_id, "Cache-Control: max-age=60", "new")?;
    stream.flush()?;
    Ok((first_params, second_params))
}
