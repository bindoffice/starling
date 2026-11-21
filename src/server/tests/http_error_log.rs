use super::support::*;
use super::*;

#[tokio::test]
async fn selects_the_inherited_or_overridden_http_error_logger() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            error_log logs/http.log info;
            server {
                listen 127.0.0.1:18362;
                location /inherited {
                    return 204;
                }
                location /override {
                    error_log logs/location.log info;
                    return 204;
                }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let state = test_state(&config);
    let group = build_listener_groups(&config)[0].clone();
    let remote = SocketAddr::from(([127, 0, 0, 1], 45678));
    let connection =
        ConnectionAddresses::direct(ListenAddress::Inet(remote), group.address.clone());

    for (target, message) in [
        ("/inherited", "inherited location selected"),
        ("/override", "override location selected"),
    ] {
        let request = Request {
            method: "GET".into(),
            target: target.into(),
            version: "HTTP/1.1".into(),
            headers: vec![Header {
                name: "Host".into(),
                value: "localhost".into(),
            }],
            body_stream: None,
            body: Vec::new(),
            remote_addr: remote.to_string(),
            local_addr: "127.0.0.1:18362".into(),
            keep_alive: false,
        };
        let snapshot = state.config_snapshot().await;
        let action = dispatch_http_request(
            snapshot,
            &group,
            &request,
            connection.clone(),
            ConnectionRequestState::new(std::time::Instant::now(), 0),
            None,
            &state,
        )
        .await
        .expect("HTTP dispatch");
        let DispatchAction::Response(outcome) = action else {
            panic!("expected an HTTP response");
        };
        assert_eq!(outcome.response.status, 204);
        outcome.error_logger.log(LogLevel::Info, message);
    }

    let http_log = std::fs::read_to_string(prefix.join("logs/http.log")).unwrap();
    let location_log = std::fs::read_to_string(prefix.join("logs/location.log")).unwrap();
    assert!(
        http_log.contains("inherited location selected"),
        "{http_log}"
    );
    assert!(
        !http_log.contains("override location selected"),
        "{http_log}"
    );
    assert!(
        location_log.contains("override location selected"),
        "{location_log}"
    );
    assert!(
        !location_log.contains("inherited location selected"),
        "{location_log}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
