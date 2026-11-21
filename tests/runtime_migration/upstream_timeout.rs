use std::time::Duration;

use super::support::{
    RunningServer, TestDirectory, assert_status, fake_upstream_with_delay, reserve_port,
};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn maps_upstream_read_timeout_to_gateway_timeout() {
    let port = reserve_port();
    let Some(upstream) = fake_upstream_with_delay("too-late", Duration::from_millis(500)) else {
        return;
    };
    let directory = TestDirectory::new("upstream-timeout");
    let config = timeout_config(port, upstream.port);
    let server = RunningServer::start(directory, port, &config);

    let response = server.get("/slow");
    assert_status(&response, 504);
    assert!(
        response.to_ascii_lowercase().contains("gateway time-out")
            || response.to_ascii_lowercase().contains("gateway timeout"),
        "unexpected timeout response:\n{response}"
    );
}

fn timeout_config(port: u16, upstream_port: u16) -> String {
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

                    location / {{
                        proxy_pass http://127.0.0.1:{upstream_port};
                        proxy_read_timeout 100ms;
                        proxy_next_upstream off;
                    }}
                }}
            }}
        "#
    )
}
