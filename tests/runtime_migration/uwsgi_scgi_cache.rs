use std::collections::HashMap;
use std::io;
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

use super::gateway_support::{
    Gateway, configure_stream, read_gateway_params, write_gateway_response,
};
use super::support::{RunningServer, TestDirectory, assert_status, request, reserve_port};

type Params = HashMap<String, String>;
type BackgroundUpdateResult = (Params, Params);

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn refreshes_expired_uwsgi_cache_in_background_while_serving_stale() {
    run_background_update(Gateway::Uwsgi);
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn refreshes_expired_scgi_cache_in_background_while_serving_stale() {
    run_background_update(Gateway::Scgi);
}

fn run_background_update(gateway: Gateway) {
    let port = reserve_port();
    let Some(upstream) = fake_background_upstream(gateway) else {
        return;
    };
    let directory = TestDirectory::new(&format!("{}-cache-background-update", gateway.directive()));
    let config = background_update_config(gateway, port, upstream.port);
    let mut server = RunningServer::start(directory, port, &config);

    let target = "/app/index.php?mode=background";
    let first = server.get(target);
    assert_status(&first, 200);
    assert_cache_status(&first, "miss");
    assert!(
        first.ends_with("old"),
        "unexpected first response:\n{first}"
    );

    thread::sleep(Duration::from_millis(1100));

    let (stale_sender, stale_receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = stale_sender.send(request(port, target));
    });
    upstream
        .update_started
        .recv_timeout(Duration::from_secs(5))
        .expect("expired gateway cache entry did not start a background update");
    let stale = stale_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("stale gateway response waited for the background update");
    assert_status(&stale, 200);
    assert_cache_status(&stale, "stale");
    assert!(
        stale.ends_with("old"),
        "unexpected stale response:\n{stale}"
    );

    let updating = server.get(target);
    assert_status(&updating, 200);
    assert_cache_status(&updating, "updating");
    assert!(
        updating.ends_with("old"),
        "unexpected updating response:\n{updating}"
    );

    upstream
        .release_update
        .send(())
        .expect("release gateway background update");
    let (first_params, second_params) = upstream
        .finished
        .recv_timeout(Duration::from_secs(5))
        .expect("gateway background update did not finish")
        .expect("gateway background update failed");
    for params in [&first_params, &second_params] {
        assert_eq!(
            params.get("REQUEST_METHOD").map(String::as_str),
            Some("GET"),
            "{} request parameters:\n{params:?}",
            gateway.label()
        );
        assert_eq!(
            params.get("REQUEST_URI").map(String::as_str),
            Some(target),
            "{} request URI:\n{params:?}",
            gateway.label()
        );
    }

    let mut last = String::new();
    for _ in 0..100 {
        thread::sleep(Duration::from_millis(20));
        last = server.get(target);
        if last.to_ascii_lowercase().contains("x-cache-status: hit") {
            break;
        }
    }
    assert_status(&last, 200);
    assert_cache_status(&last, "hit");
    assert!(
        last.ends_with("new"),
        "gateway background update did not replace the cached body:\n{last}"
    );
    server.assert_running();
}

fn assert_cache_status(response: &str, expected: &str) {
    assert!(
        response
            .to_ascii_lowercase()
            .contains(&format!("x-cache-status: {expected}\r\n")),
        "expected cache status {expected}:\n{response}"
    );
}

struct FakeGatewayBackgroundUpdate {
    port: u16,
    update_started: Receiver<()>,
    release_update: Sender<()>,
    finished: Receiver<Result<BackgroundUpdateResult, String>>,
}

fn fake_background_upstream(gateway: Gateway) -> Option<FakeGatewayBackgroundUpdate> {
    let listener = match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping runtime {} background update test: TCP binding is not permitted",
                gateway.label()
            );
            return None;
        }
        Err(error) => panic!("bind fake {} upstream: {error}", gateway.label()),
    };
    let port = listener.local_addr().unwrap().port();
    let (update_started, update_started_rx) = mpsc::channel();
    let (release_update, release_update_rx) = mpsc::channel();
    let (finished, finished_rx) = mpsc::channel();
    thread::spawn(move || {
        let result = serve_background_update(listener, gateway, update_started, release_update_rx);
        let _ = finished.send(result.map_err(|error| error.to_string()));
    });
    Some(FakeGatewayBackgroundUpdate {
        port,
        update_started: update_started_rx,
        release_update,
        finished: finished_rx,
    })
}

fn serve_background_update(
    listener: TcpListener,
    gateway: Gateway,
    update_started: Sender<()>,
    release_update: Receiver<()>,
) -> io::Result<BackgroundUpdateResult> {
    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let first_params = read_gateway_params(gateway, &mut stream)?;
    write_gateway_response(&mut stream, "", "old")?;
    drop(stream);

    let (mut stream, _) = listener.accept()?;
    configure_stream(&stream)?;
    let second_params = read_gateway_params(gateway, &mut stream)?;
    update_started
        .send(())
        .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "gateway test exited"))?;
    release_update
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "gateway background update was not released",
            )
        })?;
    write_gateway_response(&mut stream, "Cache-Control: max-age=60", "new")?;
    Ok((first_params, second_params))
}

fn background_update_config(gateway: Gateway, port: u16, upstream_port: u16) -> String {
    let kind = gateway.directive();
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
                {kind}_cache_path cache levels=1:2
                                  keys_zone={kind}_runtime:1m;

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location /app/ {{
                        {kind}_pass 127.0.0.1:{upstream_port};
                        {kind}_param REQUEST_METHOD $request_method;
                        {kind}_param REQUEST_URI $request_uri;
                        {kind}_param QUERY_STRING $query_string;
                        {kind}_cache {kind}_runtime;
                        {kind}_cache_key "$request_method$host$request_uri";
                        {kind}_cache_valid 200 1s;
                        {kind}_cache_use_stale updating;
                        {kind}_cache_background_update on;
                        {kind}_cache_lock on;
                        add_header X-Cache-Status $upstream_cache_status always;
                    }}
                }}
            }}
        "#
    )
}
