use super::super::support::*;
use crate::config::SyslogServer;
#[cfg(unix)]
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

fn bind_udp_socket() -> Option<std::net::UdpSocket> {
    match std::net::UdpSocket::bind("127.0.0.1:0") {
        Ok(socket) => Some(socket),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP syslog test: binding sockets is not permitted");
            None
        }
        Err(error) => panic!("bind UDP syslog socket: {error}"),
    }
}

#[test]
fn compiles_syslog_access_log_parameters() {
    let (_prefix, config) = test_config(
        r#"
            events {}
            http {
                log_format status '$status';
                server {
                    access_log syslog:server=127.0.0.1:5514,facility=local0,severity=warn,tag=starling,nohostname status;
                    access_log syslog:server=127.0.0.1 status;
                    access_log syslog:server=[::1]:5516 status;
                }
            }
        "#,
        &[],
    );
    let targets = &config.http.as_ref().unwrap().servers[0]
        .context
        .access_log
        .logs;

    let syslog = targets[0].destination.as_syslog().unwrap();
    assert_eq!(
        syslog.server,
        SyslogServer::Inet("127.0.0.1:5514".parse().unwrap())
    );
    assert_eq!(syslog.facility, 16);
    assert_eq!(syslog.severity, 4);
    assert_eq!(syslog.priority(), 132);
    assert_eq!(syslog.tag, "starling");
    assert!(syslog.nohostname);

    let defaults = targets[1].destination.as_syslog().unwrap();
    assert_eq!(defaults.facility, 23);
    assert_eq!(defaults.severity, 6);
    assert_eq!(defaults.priority(), 190);
    assert_eq!(defaults.tag, "starling");
    assert!(!defaults.nohostname);
    assert_eq!(
        defaults.server,
        SyslogServer::Inet("127.0.0.1:514".parse().unwrap())
    );

    let ipv6 = targets[2].destination.as_syslog().unwrap();
    assert_eq!(
        ipv6.server,
        SyslogServer::Inet("[::1]:5516".parse().unwrap())
    );
}

#[test]
fn rejects_invalid_syslog_access_log_parameters() {
    for (destination, expected) in [
        ("syslog:facility=local0", "no syslog server specified"),
        (
            "syslog:server=127.0.0.1:5514,server=127.0.0.1:5515",
            "duplicate syslog \"server\"",
        ),
        (
            "syslog:server=127.0.0.1:5514,facility=user,facility=daemon",
            "duplicate syslog \"facility\"",
        ),
        (
            "syslog:server=127.0.0.1:5514,severity=info,severity=error",
            "duplicate syslog \"severity\"",
        ),
        (
            "syslog:server=127.0.0.1:5514,tag=first,tag=second",
            "duplicate syslog \"tag\"",
        ),
        (
            "syslog:server=127.0.0.1:5514,facility=missing",
            "unknown syslog facility",
        ),
        (
            "syslog:server=127.0.0.1:5514,severity=missing",
            "unknown syslog severity",
        ),
        (
            "syslog:server=127.0.0.1:5514,tag=invalid-tag",
            "only allows alphanumeric characters and underscore",
        ),
        (
            "syslog:server=127.0.0.1:5514,unknown=value",
            "unknown syslog parameter",
        ),
        ("syslog:server=unix:", "no path in the unix domain socket"),
        ("syslog:server=127.0.0.1:0", "invalid port in syslog server"),
        (
            "syslog:server=[127.0.0.1]:514",
            "invalid IPv6 address in syslog server",
        ),
    ] {
        let source = format!(
            r#"
                events {{}}
                http {{
                    server {{
                        access_log {destination} combined;
                    }}
                }}
            "#
        );
        let error = test_config_error(&source);
        assert!(
            error.contains(expected),
            "unexpected error for {destination}: {error}"
        );
    }

    let buffered = test_config_error(
        r#"
            events {}
            http {
                server {
                    access_log syslog:server=127.0.0.1:5514 combined buffer=4k;
                }
            }
        "#,
    );
    assert!(
        buffered.contains("logs to syslog cannot be buffered"),
        "{buffered}"
    );
}

#[tokio::test]
async fn writes_http_access_logs_to_udp_syslog() {
    let Some(socket) = bind_udp_socket() else {
        return;
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let port = socket.local_addr().unwrap().port();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                log_format status '$status';
                server {{
                    listen 127.0.0.1:18192;
                    access_log syslog:server=127.0.0.1:{port},facility=local0,severity=warn,tag=starling,nohostname status;
                    return 200 "ok\n";
                }}
            }}
        "#
    );
    let (_prefix, config) = test_config(&source, &[]);

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");

    let mut buffer = [0_u8; 2048];
    let length = socket.recv(&mut buffer).unwrap();
    let message = std::str::from_utf8(&buffer[..length]).unwrap();
    assert!(message.starts_with("<132>"), "{message}");
    assert!(message.ends_with(" starling: 200"), "{message}");
    assert!(!message.contains('\n'), "{message}");
}

#[tokio::test]
async fn writes_stream_access_logs_to_udp_syslog() {
    let Some(socket) = bind_udp_socket() else {
        return;
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let port = socket.local_addr().unwrap().port();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                log_format session '$server_name';
                access_log syslog:server=127.0.0.1:{port},tag=stream,nohostname session;
                server {{
                    listen 127.0.0.1:13322;
                    server_name syslog.test;
                    return "ok";
                }}
            }}
        "#
    );
    let (_prefix, config) = test_config(&source, &[]);

    assert_eq!(stream_response(&config, b"").await, b"ok");
    let mut buffer = [0_u8; 2048];
    let length = socket.recv(&mut buffer).unwrap();
    let message = std::str::from_utf8(&buffer[..length]).unwrap();
    assert!(message.starts_with("<190>"), "{message}");
    assert!(message.ends_with(" stream: syslog.test"), "{message}");
    assert!(!message.contains('\n'), "{message}");
}

#[cfg(unix)]
#[tokio::test]
async fn writes_http_access_logs_to_unix_syslog() {
    let socket_path = test_socket_path("syslog");
    let socket = match UnixDatagram::bind(&socket_path) {
        Ok(socket) => socket,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UNIX syslog test: binding sockets is not permitted");
            return;
        }
        Err(error) => panic!("bind UNIX syslog socket: {error}"),
    };
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                log_format status '$status';
                server {{
                    listen 127.0.0.1:18193;
                    access_log syslog:server=unix:{},facility=daemon,severity=error,tag=unix,nohostname status;
                    return 204;
                }}
            }}
        "#,
        socket_path.display()
    );
    let (_prefix, config) = test_config(&source, &[]);

    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(
        response.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{response}"
    );

    let mut buffer = [0_u8; 2048];
    let length = socket.recv(&mut buffer).unwrap();
    let message = std::str::from_utf8(&buffer[..length]).unwrap();
    assert!(message.starts_with("<27>"), "{message}");
    assert!(message.ends_with(" unix: 204"), "{message}");
    let _ = std::fs::remove_file(socket_path);
}
