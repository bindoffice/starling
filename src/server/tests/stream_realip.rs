use crate::config::{IpNetwork, RealIpSource};
use crate::server::stream::handle_stream_accepted;

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_real_ip_sources() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                set_real_ip_from 127.0.0.0/8;
                server {
                    listen 127.0.0.1:13440;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13441;
                    set_real_ip_from 10.0.0.0/8;
                    set_real_ip_from unix:;
                    set_real_ip_from 192.168.0.0/16;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(
        servers[0].real_ip_from,
        vec![RealIpSource::Network(IpNetwork {
            address: "127.0.0.0".parse().unwrap(),
            prefix: 8,
        })]
    );
    assert_eq!(
        servers[1].real_ip_from,
        vec![
            RealIpSource::Network(IpNetwork {
                address: "10.0.0.0".parse().unwrap(),
                prefix: 8,
            }),
            RealIpSource::Unix,
            RealIpSource::Network(IpNetwork {
                address: "192.168.0.0".parse().unwrap(),
                prefix: 16,
            }),
        ]
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn rejects_invalid_stream_real_ip_sources() {
    let error = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13442;
                    set_real_ip_from 127.0.0.999/8;
                    return ok;
                }
            }
        "#,
    );
    assert!(
        error.contains("invalid IP address: 127.0.0.999"),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn applies_stream_real_ip_before_access_and_variables() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                set_real_ip_from 127.0.0.0/8;
                server {
                    listen 127.0.0.1:13443 proxy_protocol;
                    allow 203.0.113.0/24;
                    deny all;
                    return "$remote_addr|$remote_port|$realip_remote_addr|$realip_remote_port|$proxy_protocol_addr|$proxy_protocol_port\n";
                }
            }
        "#,
        &[],
    );
    let response = stream_return_response(
        &config,
        "127.0.0.1:45679".parse().unwrap(),
        b"PROXY TCP4 203.0.113.7 192.0.2.10 45678 443\r\n",
    )
    .await;
    assert_eq!(
        String::from_utf8(response).unwrap(),
        "203.0.113.7|45678|127.0.0.1|45679|203.0.113.7|45678\n"
    );
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                set_real_ip_from 10.0.0.0/8;
                server {
                    listen 127.0.0.1:13444 proxy_protocol;
                    return "$remote_addr|$remote_port|$realip_remote_addr|$realip_remote_port\n";
                }
            }
        "#,
        &[],
    );
    let response = stream_return_response(
        &config,
        "127.0.0.1:45680".parse().unwrap(),
        b"PROXY TCP4 203.0.113.7 192.0.2.10 45678 443\r\n",
    )
    .await;
    assert_eq!(
        String::from_utf8(response).unwrap(),
        "127.0.0.1|45680|127.0.0.1|45680\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn trusts_unix_peers_for_stream_real_ip_rewrites() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                set_real_ip_from unix:;
                server {
                    listen 127.0.0.1:13446 proxy_protocol;
                    return "$remote_addr|$remote_port|$realip_remote_addr|$realip_remote_port\n";
                }
            }
        "#,
        &[],
    );
    let response = stream_return_response_from_address(
        &config,
        ListenAddress::Unix("/tmp/starling-client.sock".into()),
        b"PROXY TCP4 203.0.113.7 192.0.2.10 45678 443\r\n",
    )
    .await;
    assert_eq!(
        String::from_utf8(response).unwrap(),
        "203.0.113.7|45678|unix:/tmp/starling-client.sock|\n"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn forwards_stream_real_ip_in_upstream_proxy_protocol() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream realip upstream test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind stream realip backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, transport_remote) = backend.accept().await.unwrap();
        let addresses = read_proxy_protocol(&mut stream, &ListenAddress::Inet(transport_remote))
            .await
            .unwrap();
        assert_eq!(
            addresses.proxy_protocol_addr,
            Some("203.0.113.7".parse().unwrap())
        );
        assert_eq!(addresses.proxy_protocol_port, Some(45678));
        assert_eq!(
            addresses.proxy_protocol_server_addr,
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(addresses.proxy_protocol_server_port, Some(13445));
        let mut request = [0_u8; 4];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"ping");
        stream.write_all(b"pong").await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                set_real_ip_from 127.0.0.0/8;
                server {{
                    listen 127.0.0.1:13445 proxy_protocol;
                    proxy_pass {backend_address};
                    proxy_protocol on;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let local = route.address.clone();
    let state = test_state(&config);
    let transport_remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));
    client
        .write_all(
            b"PROXY TCP4 203.0.113.7 192.0.2.10 45678 443\r\n\
              ping",
        )
        .await
        .unwrap();
    let mut response = [0_u8; 4];
    tokio::time::timeout(Duration::from_secs(2), client.read_exact(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&response, b"pong");
    drop(client);
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn stream_return_response(
    config: &Arc<RuntimeConfig>,
    transport_remote: SocketAddr,
    client_input: &[u8],
) -> Vec<u8> {
    stream_return_response_from_address(config, ListenAddress::Inet(transport_remote), client_input)
        .await
}

async fn stream_return_response_from_address(
    config: &Arc<RuntimeConfig>,
    transport_remote: ListenAddress,
    client_input: &[u8],
) -> Vec<u8> {
    let route = build_stream_routes(config)[0].clone();
    let local = route.address.clone();
    let state = test_state(config);
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        transport_remote,
        local,
        route,
        state,
    ));
    client.write_all(client_input).await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    response
}
