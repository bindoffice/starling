use std::thread;
use std::time::{Duration, Instant};

use super::support::{Http1Connection, RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn applies_rewrite_changes_after_sighup_without_changing_the_listener() {
    let port = reserve_port();
    let directory = TestDirectory::new("reload");
    let first = version_config(port, "v1");
    let mut server = RunningServer::start(directory, port, &first);

    let mut connection = Http1Connection::connect(port);
    let response = connection.get("/version");
    assert_status(&response, 200);
    assert!(
        response.contains("v1"),
        "initial version missing:\n{response}"
    );

    server.write_config(&version_config(port, "v2"));
    server.reload_via_cli();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = connection.get("/version");
        if response.contains("v2") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "reload did not apply the new return value:\n{response}"
        );
        thread::sleep(Duration::from_millis(50));
    }
    server.assert_running();
}

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn rejects_invalid_config_before_sending_the_reload_signal() {
    let port = reserve_port();
    let directory = TestDirectory::new("reload-invalid-config");
    let mut server = RunningServer::start(directory, port, &version_config(port, "v1"));

    let mut connection = Http1Connection::connect(port);
    let response = connection.get("/version");
    assert_status(&response, 200);
    assert!(
        response.contains("v1"),
        "initial version missing:\n{response}"
    );

    let invalid = version_config(port, "v2").replace(
        "location = /version {",
        "location = /version {\n                        made_up_directive on;",
    );
    server.write_config(&invalid);
    let output = server.run_reload_via_cli();
    assert!(
        !output.status.success(),
        "-s reload accepted an invalid configuration"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("made_up_directive"),
        "unexpected reload error:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    server.assert_running();
    let response = connection.get("/version");
    assert_status(&response, 200);
    assert!(
        response.contains("v1"),
        "invalid reload replaced the running configuration:\n{response}"
    );
    thread::sleep(Duration::from_millis(100));
    let error_log =
        std::fs::read_to_string(server.directory_path().join("logs/error.log")).unwrap();
    assert!(
        !error_log.contains("reload failed"),
        "invalid configuration reached the running process:\n{error_log}"
    );

    server.write_config(&version_config(port, "v2"));
    server.reload_via_cli();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let response = connection.get("/version");
        if response.contains("v2") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "valid reload did not apply after rejected configuration:\n{response}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn version_config(port: u16, version: &str) -> String {
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

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location = /version {{
                        return 200 "{version}";
                    }}
                }}
            }}
        "#
    )
}
