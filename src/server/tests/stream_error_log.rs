use super::support::*;
use super::*;
use crate::config::LogDestination;

#[test]
fn inherits_and_overrides_stream_error_log_targets() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                error_log logs/stream.log notice;
                server {
                    listen 127.0.0.1:13370;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13371;
                    error_log logs/server.log warn;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let stream = config.stream.as_ref().unwrap();
    assert_eq!(stream.error_logs.len(), 1);
    assert_eq!(stream.error_logs[0].level, LogLevel::Notice);
    assert!(matches!(
        &stream.error_logs[0].destination,
        LogDestination::File(path) if path.ends_with("logs/stream.log")
    ));
    assert_eq!(stream.servers[0].error_logs.len(), 1);
    assert_eq!(stream.servers[0].error_logs[0].level, LogLevel::Notice);
    assert_eq!(stream.servers[1].error_logs.len(), 1);
    assert_eq!(stream.servers[1].error_logs[0].level, LogLevel::Warn);
    assert!(matches!(
        &stream.servers[1].error_logs[0].destination,
        LogDestination::File(path) if path.ends_with("logs/server.log")
    ));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_error_log_directives() {
    let invalid_level = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                error_log logs/stream.log invalid;
                server {
                    listen 127.0.0.1:13372;
                    return ok;
                }
            }
        "#,
    );
    assert!(invalid_level.contains("invalid log level \"invalid\""));

    let too_many_arguments = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13373;
                    error_log logs/server.log info extra;
                    return ok;
                }
            }
        "#,
    );
    assert!(too_many_arguments.contains("invalid number of arguments in \"error_log\" directive"));
}

#[tokio::test]
async fn writes_stream_session_errors_to_the_selected_server_error_log() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            error_log logs/stream.log info;
            server {
                listen 127.0.0.1:13374;
                error_log logs/server.log error;
                deny 127.0.0.0/8;
                return "must not be sent";
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let response = stream_response(&config, b"").await;
    assert!(response.is_empty());

    let server_log = std::fs::read_to_string(prefix.join("logs/server.log")).unwrap();
    let stream_log = std::fs::read_to_string(prefix.join("logs/stream.log")).unwrap_or_default();
    assert!(
        server_log.contains("stream access forbidden by rule"),
        "{server_log}"
    );
    assert!(
        !stream_log.contains("stream access forbidden by rule"),
        "{stream_log}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn stream_servers_inherit_the_stream_main_error_log() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            error_log logs/stream.log info;
            server {
                listen 127.0.0.1:13375;
                deny 127.0.0.0/8;
                return "must not be sent";
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let response = stream_response(&config, b"").await;
    assert!(response.is_empty());

    let stream_log = std::fs::read_to_string(prefix.join("logs/stream.log")).unwrap();
    assert!(
        stream_log.contains("stream access forbidden by rule"),
        "{stream_log}"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
