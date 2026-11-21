use super::*;

#[tokio::test]
async fn parses_proxy_protocol_v1_addresses() {
    let actual_remote = ListenAddress::Inet("10.0.0.1:50000".parse().unwrap());
    let (mut client, mut server) = tokio::io::duplex(4096);
    client
        .write_all(b"PROXY TCP4 203.0.113.7 127.0.0.1 45678 443\r\nGET / HTTP/1.1\r\n\r\n")
        .await
        .unwrap();
    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(
        addresses.remote,
        ListenAddress::Inet("203.0.113.7:45678".parse().unwrap())
    );
    assert_eq!(
        addresses.proxy_protocol_addr,
        Some("203.0.113.7".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_port, Some(45678));
    assert_eq!(
        addresses.proxy_protocol_server_addr,
        Some("127.0.0.1".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_server_port, Some(443));
    let mut request = [0_u8; 16];
    server.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"GET / HTTP/1.1\r\n");

    let (mut client, mut server) = tokio::io::duplex(1024);
    client.write_all(b"PROXY UNKNOWN\r\n").await.unwrap();
    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(addresses.remote, actual_remote);
    assert_eq!(addresses.proxy_protocol_addr, None);
    assert_eq!(addresses.proxy_protocol_port, None);
    assert_eq!(addresses.proxy_protocol_server_addr, None);
    assert_eq!(addresses.proxy_protocol_server_port, None);
}

#[tokio::test]
async fn parses_proxy_protocol_v2_addresses() {
    let actual_remote = ListenAddress::Inet("10.0.0.1:50000".parse().unwrap());
    let (mut client, mut server) = tokio::io::duplex(4096);
    let mut header = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x21, 0x11, 0x00, 0x0f]);
    header.extend_from_slice(&[203, 0, 113, 7, 192, 0, 2, 1]);
    header.extend_from_slice(&45678_u16.to_be_bytes());
    header.extend_from_slice(&443_u16.to_be_bytes());
    header.extend_from_slice(&[0x04, 0x00, 0x00]);
    header.extend_from_slice(b"GET / HTTP/1.1\r\n");
    client.write_all(&header).await.unwrap();

    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(
        addresses.remote,
        ListenAddress::Inet("203.0.113.7:45678".parse().unwrap())
    );
    assert_eq!(
        addresses.proxy_protocol_addr,
        Some("203.0.113.7".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_port, Some(45678));
    assert_eq!(
        addresses.proxy_protocol_server_addr,
        Some("192.0.2.1".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_server_port, Some(443));
    let mut request = [0_u8; 16];
    server.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"GET / HTTP/1.1\r\n");

    let source = "2001:db8::7".parse::<std::net::Ipv6Addr>().unwrap();
    let destination = "2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap();
    let mut header = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x21, 0x21, 0x00, 0x24]);
    header.extend_from_slice(&source.octets());
    header.extend_from_slice(&destination.octets());
    header.extend_from_slice(&45679_u16.to_be_bytes());
    header.extend_from_slice(&8443_u16.to_be_bytes());
    header.extend_from_slice(b"GET /v6 HTTP/1.1\r\n");
    client.write_all(&header).await.unwrap();

    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(
        addresses.remote,
        ListenAddress::Inet("[2001:db8::7]:45679".parse().unwrap())
    );
    assert_eq!(addresses.proxy_protocol_addr, Some(IpAddr::V6(source)));
    assert_eq!(addresses.proxy_protocol_port, Some(45679));
    assert_eq!(
        addresses.proxy_protocol_server_addr,
        Some(IpAddr::V6(destination))
    );
    assert_eq!(addresses.proxy_protocol_server_port, Some(8443));
    let mut request = [0_u8; 18];
    server.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"GET /v6 HTTP/1.1\r\n");
}

#[tokio::test]
async fn ignores_proxy_protocol_v2_local_and_unsupported_transport_headers() {
    let actual_remote = ListenAddress::Inet("10.0.0.1:50000".parse().unwrap());
    let (mut client, mut server) = tokio::io::duplex(4096);
    let mut header = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]);
    header.extend_from_slice(b"GET /local HTTP/1.1\r\n");
    client.write_all(&header).await.unwrap();
    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(addresses.remote, actual_remote);
    assert_eq!(addresses.proxy_protocol_addr, None);
    let mut request = [0_u8; 21];
    server.read_exact(&mut request).await.unwrap();
    assert_eq!(&request, b"GET /local HTTP/1.1\r\n");

    let mut header = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x21, 0x12, 0x00, 0x0c]);
    header.extend_from_slice(&[203, 0, 113, 7, 192, 0, 2, 1]);
    header.extend_from_slice(&45678_u16.to_be_bytes());
    header.extend_from_slice(&443_u16.to_be_bytes());
    header.extend_from_slice(b"GET /udp HTTP/1.1\r\n\r\n");
    client.write_all(&header).await.unwrap();
    let addresses = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap();
    assert_eq!(addresses.remote, actual_remote);
    assert_eq!(addresses.proxy_protocol_addr, None);
}

#[tokio::test]
async fn rejects_unknown_proxy_protocol_v2_versions() {
    let actual_remote = ListenAddress::Inet("10.0.0.1:50000".parse().unwrap());
    let (mut client, mut server) = tokio::io::duplex(4096);
    let mut header = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    header.extend_from_slice(&[0x31, 0x00, 0x00, 0x00]);
    client.write_all(&header).await.unwrap();
    let error = read_proxy_protocol(&mut server, &actual_remote)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("version"));
}

#[tokio::test]
async fn exposes_proxy_protocol_v2_tlv_variables() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18445 proxy_protocol;
                        location / {
                            return 200 "$proxy_protocol_tlv_alpn|$proxy_protocol_tlv_authority|$proxy_protocol_tlv_ssl_version|$proxy_protocol_tlv_ssl_verify|$proxy_protocol_tlv_0x05|$proxy_protocol_tlv_netns\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let transport_remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(transport_remote), local),
        group,
        state,
    ));

    let mut payload = Vec::new();
    payload.extend_from_slice(&[203, 0, 113, 7, 192, 0, 2, 1]);
    payload.extend_from_slice(&45678_u16.to_be_bytes());
    payload.extend_from_slice(&80_u16.to_be_bytes());
    for (kind, value) in [
        (0x01, b"h2".as_slice()),
        (0x02, b"example.test".as_slice()),
        (0x05, b"uid1".as_slice()),
    ] {
        payload.push(kind);
        payload.extend_from_slice(&(value.len() as u16).to_be_bytes());
        payload.extend_from_slice(value);
    }
    let mut ssl = vec![1, 0, 0, 0, 1];
    ssl.extend_from_slice(&[0x21, 0x00, 0x03]);
    ssl.extend_from_slice(b"TLS");
    payload.push(0x20);
    payload.extend_from_slice(&(ssl.len() as u16).to_be_bytes());
    payload.extend_from_slice(&ssl);

    let mut request = PROXY_PROTOCOL_V2_SIGNATURE.to_vec();
    request.extend_from_slice(&[0x21, 0x11]);
    request.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    request.extend_from_slice(&payload);
    request.extend_from_slice(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    client.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.ends_with("h2|example.test|TLS|1|uid1|\n"),
        "{response}"
    );
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn exposes_proxy_protocol_address_separately_from_realip_peer() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18446 proxy_protocol;
                        location / {
                            return 200 "$remote_addr|$proxy_protocol_addr|$realip_remote_addr|$proxy_protocol_port|$proxy_protocol_server_addr|$proxy_protocol_server_port\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let transport_remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(transport_remote), local),
        group,
        state,
    ));
    client
        .write_all(
            b"PROXY TCP4 203.0.113.7 192.0.2.1 45678 80\r\n\
                  GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.ends_with("127.0.0.1|203.0.113.7|127.0.0.1|45678|192.0.2.1|80\n"),
        "{response}"
    );
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18447;
                        location / {
                            return 200 "$proxy_protocol_addr|$proxy_protocol_port|$realip_remote_addr\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let response = request_body(
        &config,
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.ends_with("||127.0.0.1\n"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}
