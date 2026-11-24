use super::*;

#[test]
fn request_limit_uses_signed_rate_decay_before_checking_burst() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    limit_req_zone $arg_key zone=fast:1m rate=1000r/s;
                    server {
                        listen 127.0.0.1:18116;
                        location / {
                            limit_req zone=fast;
                            return 204;
                        }
                    }
                }
            "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let limit = &server.locations[0].context.limit_reqs[0];
    let mut state = LimitState::default();
    let now = Instant::now();

    assert!(matches!(
        consume_request_limit(&mut state, limit, b"key", now),
        LimitReqDecision::Passed
    ));
    assert!(matches!(
        consume_request_limit(&mut state, limit, b"key", now + Duration::from_millis(100)),
        LimitReqDecision::Passed
    ));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_limit_req_dry_run_status_without_delaying_or_rejecting() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    error_log logs/error.log info;
                    limit_req_zone $uri zone=one:1m rate=1r/m;
                    log_format test '$uri:$limit_req_status';
                    server {
                        listen 127.0.0.1:18116;
                        limit_req_dry_run on;
                        access_log logs/test.log test;
                        location /delay {
                            limit_req zone=one burst=2;
                            return 200 "$limit_req_status\n";
                        }
                        location /reject {
                            limit_req zone=one;
                            return 200 "$limit_req_status\n";
                        }
                        location / { return 204; }
                    }
                }
            "#,
        &[],
    );
    let state = test_state(&config);
    let request = |path: &str| {
        let state = state.clone();
        let config = config.clone();
        let path = path.to_string();
        async move {
            request_body_with_state(
                &config,
                state,
                &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
            )
            .await
        }
    };

    let passed = request("/delay").await;
    assert!(
        passed.starts_with("HTTP/1.1 200 OK\r\n") && passed.ends_with("PASSED\n"),
        "{passed}"
    );
    let started = Instant::now();
    let delayed = request("/delay").await;
    assert!(
        delayed.starts_with("HTTP/1.1 200 OK\r\n") && delayed.ends_with("DELAYED_DRY_RUN\n"),
        "{delayed}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "dry-run delay slept for {:?}",
        started.elapsed()
    );
    let _ = request("/reject").await;
    let rejected = request("/reject").await;
    assert!(
        rejected.starts_with("HTTP/1.1 200 OK\r\n") && rejected.ends_with("REJECTED_DRY_RUN\n"),
        "{rejected}"
    );
    let _ = request("/").await;

    let error_log = std::fs::read_to_string(prefix.join("logs/error.log")).unwrap();
    assert!(
        error_log.contains("delaying request, dry run, excess:")
            && error_log.contains("by zone \"one\""),
        "{error_log}"
    );
    assert!(
        error_log.contains("limiting requests, dry run, excess:")
            && error_log.contains("by zone \"one\""),
        "{error_log}"
    );
    let access_log = std::fs::read_to_string(prefix.join("logs/test.log")).unwrap();
    assert!(access_log.lines().any(|line| line == "/:-"), "{access_log}");
    let _ = std::fs::remove_dir_all(prefix);
}
