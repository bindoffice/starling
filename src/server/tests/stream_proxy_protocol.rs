use crate::config::StreamProxyProtocol;
use crate::server::stream::{
    StreamProxyProtocolTransport, build_stream_proxy_protocol_header, handle_stream_accepted,
};

use super::support::*;
use super::*;

#[test]
fn compiles_and_inherits_stream_upstream_proxy_protocol() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_protocol on;
                server {
                    listen 127.0.0.1:13340;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13341;
                    proxy_protocol v2;
                    return overridden;
                }
                server {
                    listen 127.0.0.1:13342;
                    proxy_protocol off;
                    return disabled;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_protocol, StreamProxyProtocol::V1);
    assert_eq!(servers[1].proxy_protocol, StreamProxyProtocol::V2);
    assert_eq!(servers[2].proxy_protocol, StreamProxyProtocol::Off);
    let _ = std::fs::remove_dir_all(prefix);

    let invalid = test_config_error(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_protocol invalid;
                server { listen 127.0.0.1:13340; return ok; }
            }
        "#,
    );
    assert!(invalid.contains("invalid value \"invalid\" in \"proxy_protocol\" directive"));
}

#[test]
fn compiles_and_inherits_stream_proxy_protocol_timeout() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                proxy_protocol_timeout 45s;
                server {
                    listen 127.0.0.1:13340 proxy_protocol;
                    return inherited;
                }
                server {
                    listen 127.0.0.1:13341 proxy_protocol;
                    proxy_protocol_timeout 750ms;
                    return overridden;
                }
            }
        "#,
        &[],
    );
    let servers = &config.stream.as_ref().unwrap().servers;
    assert_eq!(servers[0].proxy_protocol_timeout, Duration::from_secs(45));
    assert_eq!(
        servers[1].proxy_protocol_timeout,
        Duration::from_millis(750)
    );
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13342 proxy_protocol;
                    return default;
                }
            }
        "#,
        &[],
    );
    assert_eq!(
        config.stream.as_ref().unwrap().servers[0].proxy_protocol_timeout,
        Duration::from_secs(30)
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test(start_paused = true)]
async fn closes_stream_when_proxy_protocol_header_exceeds_timeout() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            stream {
                server {
                    listen 127.0.0.1:13343 proxy_protocol;
                    proxy_protocol_timeout 50ms;
                    return unreachable;
                }
            }
        "#,
        &[],
    );
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let (client, inbound) = tokio::io::duplex(64);
    let remote = ListenAddress::Inet("127.0.0.1:50000".parse().unwrap());
    let local = ListenAddress::Inet("127.0.0.1:13343".parse().unwrap());
    let task = tokio::spawn(handle_stream_accepted(inbound, remote, local, route, state));

    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_millis(49)).await;
    tokio::task::yield_now().await;
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_millis(1)).await;
    tokio::time::timeout(Duration::from_millis(1), task)
        .await
        .unwrap()
        .unwrap();
    drop(client);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn builds_v1_and_v2_upstream_proxy_protocol_headers() {
    let source = ListenAddress::Inet("203.0.113.20:45678".parse().unwrap());
    let destination = ListenAddress::Inet("127.0.0.1:13340".parse().unwrap());
    let remote = ListenAddress::Inet("127.0.0.1:50000".parse().unwrap());

    let v1 = build_stream_proxy_protocol_header(
        StreamProxyProtocol::V1,
        &source,
        &destination,
        StreamProxyProtocolTransport::Stream,
    );
    assert_eq!(v1, b"PROXY TCP4 203.0.113.20 127.0.0.1 45678 13340\r\n");
    let addresses = parse_proxy_header(&v1, &remote).await;
    assert_eq!(
        addresses.proxy_protocol_addr,
        Some("203.0.113.20".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_port, Some(45678));
    assert_eq!(
        addresses.proxy_protocol_server_addr,
        Some("127.0.0.1".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_server_port, Some(13340));

    let source6 = ListenAddress::Inet("[2001:db8::1]:45678".parse().unwrap());
    let destination6 = ListenAddress::Inet("[2001:db8::2]:13340".parse().unwrap());
    let v2 = build_stream_proxy_protocol_header(
        StreamProxyProtocol::V2,
        &source6,
        &destination6,
        StreamProxyProtocolTransport::Stream,
    );
    assert_eq!(&v2[..12], &PROXY_PROTOCOL_V2_SIGNATURE);
    assert_eq!(v2[12], 0x21);
    assert_eq!(v2[13], 0x21);
    assert_eq!(u16::from_be_bytes([v2[14], v2[15]]), 43);
    assert_eq!(&v2[v2.len() - 7..v2.len() - 4], &[0x03, 0x00, 0x04]);
    let advertised_crc = u32::from_be_bytes(v2[v2.len() - 4..].try_into().unwrap());
    let mut zeroed = v2.clone();
    zeroed[v2.len() - 4..].fill(0);
    assert_eq!(advertised_crc, test_crc32c(&zeroed));

    let addresses = parse_proxy_header(&v2, &remote).await;
    assert_eq!(
        addresses.proxy_protocol_addr,
        Some("2001:db8::1".parse().unwrap())
    );
    assert_eq!(
        addresses.proxy_protocol_server_addr,
        Some("2001:db8::2".parse().unwrap())
    );
}

#[tokio::test]
async fn sends_upstream_proxy_protocol_before_tcp_payload() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(backend) => backend,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping stream upstream PROXY protocol test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind stream PROXY protocol backend: {err}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let backend_task = tokio::spawn(async move {
        let (mut stream, transport_remote) = backend.accept().await.unwrap();
        let addresses = read_proxy_protocol(&mut stream, &ListenAddress::Inet(transport_remote))
            .await
            .unwrap();
        assert_eq!(
            addresses.proxy_protocol_addr,
            Some("203.0.113.21".parse().unwrap())
        );
        assert_eq!(addresses.proxy_protocol_port, Some(45678));
        assert_eq!(
            addresses.proxy_protocol_server_addr,
            Some("127.0.0.1".parse().unwrap())
        );
        assert_eq!(addresses.proxy_protocol_server_port, Some(13343));
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
                server {{
                    listen 127.0.0.1:13343;
                    proxy_pass {backend_address};
                    proxy_protocol on;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let remote: SocketAddr = "203.0.113.21:45678".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(1024);
    let task = tokio::spawn(async move {
        let addresses = ProxyProtocolAddresses::without_header(&ListenAddress::Inet(remote));
        handle_stream_connection(inbound, &addresses, route, state).await;
    });
    client.write_all(b"ping").await.unwrap();
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

#[tokio::test]
async fn sends_v2_datagram_proxy_protocol_before_udp_payload() {
    let upstream = match UdpSocket::bind("127.0.0.1:0").await {
        Ok(upstream) => upstream,
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping UDP upstream PROXY protocol test: sockets not permitted");
            return;
        }
        Err(err) => panic!("bind UDP PROXY protocol upstream: {err}"),
    };
    let listener = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let listen_address = listener.local_addr().unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let client_address = client.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let mut datagram = [0_u8; u16::MAX as usize];
        let (size, _source) = upstream.recv_from(&mut datagram).await.unwrap();
        assert_eq!(&datagram[..12], &PROXY_PROTOCOL_V2_SIGNATURE);
        assert_eq!(datagram[12], 0x21);
        assert_eq!(datagram[13], 0x12);
        let payload = &datagram[16..size];
        let IpAddr::V4(client_ip) = client_address.ip() else {
            unreachable!("UDP test uses an IPv4 client");
        };
        let IpAddr::V4(listen_ip) = listen_address.ip() else {
            unreachable!("UDP test uses an IPv4 listener");
        };
        assert_eq!(&payload[..4], &client_ip.octets());
        assert_eq!(&payload[4..8], &listen_ip.octets());
        assert_eq!(
            u16::from_be_bytes([payload[8], payload[9]]),
            client_address.port()
        );
        assert_eq!(
            u16::from_be_bytes([payload[10], payload[11]]),
            listen_address.port()
        );

        let (size, source) = upstream.recv_from(&mut datagram).await.unwrap();
        assert_eq!(&datagram[..size], b"ping");
        upstream.send_to(b"pong", source).await.unwrap();
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            stream {{
                server {{
                    listen {listen_address} udp;
                    proxy_pass {upstream_address};
                    proxy_protocol v2;
                    proxy_timeout 5s;
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let route = build_stream_routes(&config)[0].clone();
    let state = test_state(&config);
    let listener = Arc::new(listener);
    let (_shutdown_sender, shutdown) = watch::channel(ShutdownMode::Running);
    let (sender, receiver) = mpsc::channel(4);
    let session = tokio::spawn(handle_stream_udp_session(
        listener,
        receiver,
        client_address,
        route,
        state,
        shutdown,
    ));
    sender.send(b"ping".to_vec()).await.unwrap();
    let mut response = [0_u8; 16];
    let (size, from) =
        tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response))
            .await
            .unwrap()
            .unwrap();
    assert_eq!(from, listen_address);
    assert_eq!(&response[..size], b"pong");
    drop(sender);
    tokio::time::timeout(Duration::from_secs(2), session)
        .await
        .unwrap()
        .unwrap();
    upstream_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn parse_proxy_header(header: &[u8], remote: &ListenAddress) -> ProxyProtocolAddresses {
    let (mut writer, mut reader) = tokio::io::duplex(4096);
    writer.write_all(header).await.unwrap();
    writer.shutdown().await.unwrap();
    read_proxy_protocol(&mut reader, remote).await.unwrap()
}

fn test_crc32c(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0x82f6_3b78
            };
        }
    }
    !crc
}
