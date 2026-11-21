use super::support::{
    RunningServer, TestDirectory, assert_status, fake_upstream, fake_upstream_with_status,
    reserve_port,
};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn retries_the_next_named_upstream_after_http_500() {
    let port = reserve_port();
    let Some(failing) = fake_upstream_with_status("first-backend-failed", 500) else {
        return;
    };
    let Some(healthy) = fake_upstream("second-backend-ok") else {
        return;
    };
    let directory = TestDirectory::new("upstream-failover");
    let config = failover_config(port, failing.port, healthy.port);
    let server = RunningServer::start(directory, port, &config);

    let response = server.get("/failover");
    assert_status(&response, 200);
    assert!(
        response.contains("second-backend-ok"),
        "healthy backend response missing:\n{response}"
    );
    assert!(
        !response.contains("first-backend-failed"),
        "failed backend response leaked to client:\n{response}"
    );

    let failing_request = failing.captured();
    let healthy_request = healthy.captured();
    assert!(
        failing_request.starts_with("GET /failover HTTP/1.1\r\n"),
        "first backend did not receive the request:\n{failing_request}"
    );
    assert!(
        healthy_request.starts_with("GET /failover HTTP/1.1\r\n"),
        "retry backend did not receive the request:\n{healthy_request}"
    );
}

fn failover_config(port: u16, failing_port: u16, healthy_port: u16) -> String {
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

                upstream runtime_backend {{
                    server 127.0.0.1:{failing_port};
                    server 127.0.0.1:{healthy_port};
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;

                    location / {{
                        proxy_pass http://runtime_backend;
                        proxy_next_upstream error timeout http_500;
                        proxy_next_upstream_tries 2;
                    }}
                }}
            }}
        "#
    )
}
