use super::support::*;
use super::*;

async fn pass_response(config: &Arc<RuntimeConfig>, source_port: u16, input: &[u8]) -> Vec<u8> {
    let route = build_stream_routes(config)
        .into_iter()
        .find(|route| route.address.port() == source_port)
        .expect("source stream route");
    let state = test_state(config);
    let remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
    let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
    let (mut client, inbound) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    client.write_all(input).await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    response
}

#[test]
fn compiles_static_and_variable_stream_pass_targets() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:14000;
                    pass 127.0.0.1:14001;
                }
                server {
                    listen 127.0.0.1:14001;
                    return target;
                }
                server {
                    listen 127.0.0.1:14002;
                    set $destination 127.0.0.1:14003;
                    pass $destination;
                }
                server {
                    listen 127.0.0.1:14003;
                    return variable;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].handler, StreamHandler::Pass);
    assert_eq!(
        servers[0].pass.as_ref().unwrap().address,
        Some(ListenAddress::Inet("127.0.0.1:14001".parse().unwrap()))
    );
    assert!(servers[2].pass.as_ref().unwrap().address.is_none());
    assert_eq!(servers[2].pass.as_ref().unwrap().target, "$destination");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn stream_handler_follows_directive_order() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:14004;
                    pass 127.0.0.1:14006;
                    proxy_pass 127.0.0.1:15460;
                }
                server {
                    listen 127.0.0.1:14005;
                    return target;
                }
                server {
                    listen 127.0.0.1:14006;
                    return target;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].handler, StreamHandler::Proxy);
    assert!(servers[0].pass.is_some());
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_pass_directives() {
    for (body, expected) in [
        ("pass;", "invalid number of arguments in \"pass\" directive"),
        (
            "pass 127.0.0.1:14010; pass 127.0.0.1:14011;",
            "\"pass\" directive is duplicate",
        ),
        (
            "pass localhost:14010;",
            "has no addresses in \"localhost:14010\" of the \"pass\" directive",
        ),
        (
            "pass 127.0.0.1;",
            "has no port in \"127.0.0.1\" of the \"pass\" directive",
        ),
        (
            "pass 127.0.0.1:0;",
            "invalid port in \"127.0.0.1:0\" of the \"pass\" directive",
        ),
    ] {
        let source = format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                stream {{
                    server {{
                        listen 127.0.0.1:14009;
                        {body}
                    }}
                }}
            "#
        );
        let error = test_config_error(&source);
        assert!(error.contains(expected), "{error}");
    }
}

#[tokio::test]
async fn passes_connection_to_local_listener_and_logs_only_target() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            log_format pass '$server_name $status';
            access_log logs/pass.log pass;
            server {
                listen 127.0.0.1:14020;
                server_name source;
                pass 127.0.0.1:14021;
            }
            server {
                listen 127.0.0.1:14021;
                server_name target;
                return "$server_name|$server_addr:$server_port";
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let response = pass_response(&config, 14020, b"").await;
    assert_eq!(
        response,
        b"target|127.0.0.1:14021".as_slice(),
        "{}",
        String::from_utf8_lossy(&response)
    );
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/pass.log")).unwrap(),
        "target 200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn evaluates_variable_pass_target() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            server {
                listen 127.0.0.1:14022;
                set $destination 127.0.0.1:14023;
                pass $destination;
            }
            server {
                listen 127.0.0.1:14023;
                return variable-target;
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    assert_eq!(pass_response(&config, 14022, b"").await, b"variable-target");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn passes_to_wildcard_local_listener() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            server {
                listen 127.0.0.1:14024;
                pass 127.0.0.1:14025;
            }
            server {
                listen 0.0.0.0:14025;
                return "$server_addr:$server_port";
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    assert_eq!(pass_response(&config, 14024, b"").await, b"127.0.0.1:14025");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn handles_missing_target_and_pass_cycles() {
    let missing = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            log_format pass '$status';
            access_log logs/missing.log pass;
            server {
                listen 127.0.0.1:14026;
                pass 127.0.0.1:14027;
            }
        }
    "#;
    let (prefix, config) = test_config(missing, &[]);
    assert!(pass_response(&config, 14026, b"").await.is_empty());
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/missing.log")).unwrap(),
        "200\n"
    );
    let _ = std::fs::remove_dir_all(prefix);

    let cycle = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            log_format pass '$status';
            access_log logs/cycle.log pass;
            server {
                listen 127.0.0.1:14028;
                pass 127.0.0.1:14029;
            }
            server {
                listen 127.0.0.1:14029;
                pass 127.0.0.1:14028;
            }
        }
    "#;
    let (prefix, config) = test_config(cycle, &[]);
    assert!(pass_response(&config, 14028, b"").await.is_empty());
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/cycle.log")).unwrap(),
        "500\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_pass_with_preread_data() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            log_format pass '$status';
            access_log logs/preread.log pass;
            server {
                listen 127.0.0.1:14030;
                ssl_preread on;
                pass 127.0.0.1:14031;
            }
            server {
                listen 127.0.0.1:14031;
                return must-not-run;
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let hello = stream_client_hello("pass.test", &[]);
    assert!(pass_response(&config, 14030, &hello).await.is_empty());
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/preread.log")).unwrap(),
        "500\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn target_listener_applies_its_own_proxy_protocol_handler() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        stream {
            server {
                listen 127.0.0.1:14032;
                pass 127.0.0.1:14033;
            }
            server {
                listen 127.0.0.1:14033 proxy_protocol;
                return "$proxy_protocol_addr|$remote_addr";
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let response = pass_response(
        &config,
        14032,
        b"PROXY TCP4 203.0.113.9 127.0.0.1 4567 14033\r\nhello",
    )
    .await;
    assert_eq!(response, b"203.0.113.9|127.0.0.1");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_stream_pass_for_udp_listener() {
    let listener = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => return,
        Err(err) => panic!("bind UDP stream listener: {err}"),
    };
    let listen_address = listener.local_addr().unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                log_format pass '$protocol $status';
                access_log logs/udp-pass.log pass;
                server {{
                    listen {listen_address} udp;
                    pass 127.0.0.1:14034;
                }}
                server {{
                    listen 127.0.0.1:14034;
                    return tcp-target;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)
        .into_iter()
        .find(|route| route.address == ListenAddress::Inet(listen_address))
        .unwrap();
    let (sender, receiver) = mpsc::channel(4);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let session = tokio::spawn(handle_stream_udp_session(
        Arc::new(listener),
        receiver,
        client.local_addr().unwrap(),
        route,
        test_state(&config),
        shutdown,
    ));
    sender.send(b"ignored".to_vec()).await.unwrap();
    session.await.unwrap();
    assert_eq!(
        std::fs::read_to_string(prefix.join("logs/udp-pass.log")).unwrap(),
        "UDP 500\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
