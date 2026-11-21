use std::net::Ipv4Addr;

use crate::config::IpNetwork;

use super::support::*;
use super::*;

fn stream_return_config(server_body: &str) -> String {
    format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:13313;
                        {server_body}
                    }}
                }}
            "#
    )
}

#[test]
fn compiles_stream_return_without_proxy_pass() {
    let (prefix, config) = test_config(
        &stream_return_config(
            r#"server_name Return.Test;
               return "$remote_addr|$server_name";"#,
        ),
        &[],
    );
    let server = &config.stream.as_ref().unwrap().servers[0];
    assert_eq!(server.server_name, "return.test");
    assert_eq!(
        server.return_value.as_deref(),
        Some("$remote_addr|$server_name")
    );
    assert!(server.proxy_pass.is_none());
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_return_directives() {
    let duplicate = test_config_error(&stream_return_config("return first; return second;"));
    assert!(duplicate.contains("\"return\" directive is duplicate"));

    let missing_argument = test_config_error(&stream_return_config("return;"));
    assert!(missing_argument.contains("invalid number of arguments in \"return\" directive"));

    let too_many_arguments = test_config_error(&stream_return_config("return first second;"));
    assert!(too_many_arguments.contains("invalid number of arguments in \"return\" directive"));
}

#[test]
fn rejects_unimplemented_stream_control_directives() {
    let conditional = test_config_error(&stream_return_config("if ($remote_addr) { return ok; }"));
    assert!(conditional.contains("\"if\" is not supported in stream server context"));
}

#[test]
fn compiles_stream_set_directives() {
    let (prefix, config) = test_config(
        &stream_return_config(
            r#"set $name value;
               set $message "$name:$remote_addr";
               return "$message";"#,
        ),
        &[],
    );
    let server = &config.stream.as_ref().unwrap().servers[0];
    assert_eq!(server.sets.len(), 2);
    assert_eq!(server.sets[0].name, "name");
    assert_eq!(server.sets[0].value, "value");
    assert_eq!(server.sets[1].name, "message");
    assert_eq!(server.sets[1].value, "$name:$remote_addr");
    let _ = std::fs::remove_dir_all(prefix);

    let invalid_name = test_config_error(&stream_return_config("set name value; return ok;"));
    assert!(invalid_name.contains("invalid variable name in \"set\" directive"));
}

#[test]
fn inherits_and_overrides_stream_access_rules() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    deny 10.0.0.0/8;
                    server {
                        listen 127.0.0.1:13314;
                        return inherited;
                    }
                    server {
                        listen 127.0.0.1:13315;
                        allow 192.0.2.0/24;
                        deny all;
                        return overridden;
                    }
                }
            "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert!(matches!(
        servers[0].access_rules.as_slice(),
        [AccessRule::Deny(IpNetwork {
            address: IpAddr::V4(address),
            prefix: 8
        })] if *address == "10.0.0.0".parse::<Ipv4Addr>().unwrap()
    ));
    assert!(matches!(
        servers[1].access_rules.as_slice(),
        [
            AccessRule::Allow(IpNetwork {
                address: IpAddr::V4(address),
                prefix: 24
            }),
            AccessRule::DenyAll
        ] if *address == "192.0.2.0".parse::<Ipv4Addr>().unwrap()
    ));
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn validates_stream_access_networks() {
    let invalid = test_config_error(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    allow 10.0.0.0/99;
                    server {
                        listen 127.0.0.1:13316;
                        return ok;
                    }
                }
            "#,
    );
    assert!(invalid.contains("invalid CIDR prefix: 99"));
}

#[test]
fn stream_access_rules_use_first_match_semantics() {
    let rules = vec![
        AccessRule::Deny(IpNetwork {
            address: "10.0.0.0".parse().unwrap(),
            prefix: 8,
        }),
        AccessRule::AllowAll,
    ];
    assert!(!access_allowed(&rules, "10.1.2.3"));
    assert!(access_allowed(&rules, "192.0.2.1"));

    assert!(!access_allowed(&[AccessRule::DenyAll], "192.0.2.1"));
    assert!(access_allowed(&[AccessRule::AllowAll], "192.0.2.1"));
}

#[tokio::test]
async fn writes_stream_return_and_expands_session_variables() {
    let source = r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    server {
                        listen 127.0.0.1:13317;
                        server_name return.test;
                        set $greeting "hello";
                        set $message "$greeting:$remote_addr:$remote_port";
                        return "$message|$server_addr:$server_port|$server_name|$protocol\n";
                    }
                }
            "#;
    let (prefix, config) = test_config(source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.7:45679".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert_eq!(
        response,
        b"hello:203.0.113.7:45679|127.0.0.1:13317|return.test|TCP\n"
    );
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn applies_stream_access_log_if_filters() {
    for (condition, expected) in [
        ("$upstream_addr", None),
        ("$server_name", Some("filter.test 200\n")),
    ] {
        let source = format!(
            r#"
                    worker_processes 1;
                    events {{ worker_connections 64; }}
                    stream {{
                        log_format filter '$server_name $status';
                        access_log logs/filter.log filter if={condition};
                        server {{
                            listen 127.0.0.1:13319;
                            server_name filter.test;
                            return "ok";
                        }}
                    }}
                "#
        );
        let (prefix, config) = test_config(&source, &[]);
        let response = stream_response(&config, b"").await;
        assert_eq!(response, b"ok");
        let logged = std::fs::read_to_string(prefix.join("logs/filter.log")).unwrap_or_default();
        assert_eq!(logged.as_str(), expected.unwrap_or_default(), "{condition}");
        let _ = std::fs::remove_dir_all(prefix);
    }
}

#[tokio::test]
async fn rejects_stream_access_before_return_or_proxy() {
    let source = r#"
                worker_processes 1;
                events { worker_connections 64; }
                stream {
                    log_format control '$status';
                    server {
                        listen 127.0.0.1:13318;
                        access_log logs/control.log control;
                        deny 127.0.0.0/8;
                        return "must not be sent";
                    }
                }
            "#;
    let (prefix, config) = test_config(source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    assert!(response.is_empty());
    task.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/control.log")).unwrap(),
        "403\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn returns_stream_text_over_udp() {
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(socket) => socket,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP stream return test: binding sockets is not permitted");
            return;
        }
        Err(err) => panic!("bind UDP stream listener: {err}"),
    };
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_address = listener.local_addr().unwrap();
    let client_address = client.local_addr().unwrap();
    let source = format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    log_format control '$protocol $status $bytes_sent';
                    server {{
                        listen {listen_address} udp;
                        access_log logs/udp-return.log control;
                        return "$remote_addr|$remote_port|$protocol";
                    }}
                }}
            "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let listener = Arc::new(listener);
    let (sender, receiver) = mpsc::channel(4);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        listener,
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));
    sender.send(b"ignored".to_vec()).await.unwrap();
    let mut response = [0_u8; 128];
    let (size, from) =
        tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, listen_address);
    assert_eq!(
        std::str::from_utf8(&response[..size]).unwrap(),
        format!("{}|{}|UDP", client_address.ip(), client_address.port())
    );
    session.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/udp-return.log")).unwrap(),
        format!("UDP 200 {size}\n")
    );
    let _ = std::fs::remove_dir_all(prefix);
}
