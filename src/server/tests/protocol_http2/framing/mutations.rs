use super::*;

const CASES: u64 = 128;
const MALFORMED_CONNECTIONS: u64 = 32;
const HEALTHY_CONNECTIONS: u64 = 8;
const HEALTHY_REQUESTS: usize = 8;

#[tokio::test]
async fn deterministic_malformed_frame_mutations_terminate_cleanly() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18464 http2;
                location / { return 204; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();

    for case in 0..CASES {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64 ^ case;
        let malformed = malformed_frame(case, &mut seed);
        let mut bytes = Vec::new();
        if case % 3 == 0 {
            // Exercise malformed frames interleaved with a valid in-flight request.
            bytes.extend(frame(
                FRAME_HEADERS,
                FLAG_END_HEADERS,
                1,
                b"\x83\x86\x84\x01\x09localhost",
            ));
        }
        bytes.extend(malformed);

        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let server_task = tokio::spawn(handle_http_connection(
            server,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local.clone()),
            group.clone(),
            state.clone(),
        ));
        client
            .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
            .await
            .unwrap();
        if case % 2 == 0 {
            client
                .write_all(&frame(FRAME_SETTINGS, 0, 0, &[]))
                .await
                .unwrap();
        }
        client.write_all(&bytes).await.unwrap();
        client.flush().await.unwrap();

        tokio::time::timeout(Duration::from_secs(2), wait_for_terminal(&mut client))
            .await
            .unwrap_or_else(|_| panic!("malformed frame case {case} did not terminate"));
        drop(client);
        tokio::time::timeout(Duration::from_secs(2), server_task)
            .await
            .unwrap_or_else(|_| panic!("malformed frame case {case} left the handler running"))
            .unwrap_or_else(|error| panic!("malformed frame case {case} panicked: {error}"));
    }

    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_malformed_connections_do_not_disrupt_healthy_http2_clients() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 128; }
        http {
            server {
                listen 127.0.0.1:18464 http2;
                server_name localhost;
                location / { return 200 "healthy"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(
        (MALFORMED_CONNECTIONS + HEALTHY_CONNECTIONS) as usize,
    ));

    for case in 0..MALFORMED_CONNECTIONS {
        let group = group.clone();
        let state = state.clone();
        let local = local.clone();
        let start = start.clone();
        tasks.spawn(async move {
            start.wait().await;
            run_malformed_connection(case, group, state, local, remote).await;
        });
    }
    for client in 0..HEALTHY_CONNECTIONS {
        let group = group.clone();
        let state = state.clone();
        let local = local.clone();
        let start = start.clone();
        tasks.spawn(async move {
            start.wait().await;
            run_healthy_connection(client, group, state, local, remote).await;
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap_or_else(|error| panic!("concurrent HTTP/2 task panicked: {error}"));
    }
    let _ = std::fs::remove_dir_all(prefix);
}

async fn run_malformed_connection(
    case: u64,
    group: ListenerGroup,
    state: SharedState,
    local: ListenAddress,
    remote: SocketAddr,
) {
    let mut seed = 0xa076_1d64_78bd_642f_u64 ^ case;
    let malformed = malformed_frame(case, &mut seed);
    let mut bytes = Vec::new();
    if case.is_multiple_of(3) {
        bytes.extend(frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS,
            1,
            b"\x83\x86\x84\x01\x09localhost",
        ));
    }
    bytes.extend(malformed);

    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n")
        .await
        .unwrap();
    if case.is_multiple_of(2) {
        client
            .write_all(&frame(FRAME_SETTINGS, 0, 0, &[]))
            .await
            .unwrap();
    }
    client.write_all(&bytes).await.unwrap();
    client.flush().await.unwrap();

    tokio::time::timeout(Duration::from_secs(3), wait_for_terminal(&mut client))
        .await
        .unwrap_or_else(|_| panic!("concurrent malformed case {case} did not terminate"));
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap_or_else(|_| panic!("concurrent malformed case {case} left its handler running"))
        .unwrap_or_else(|error| panic!("concurrent malformed case {case} panicked: {error}"));
}

async fn run_healthy_connection(
    client: u64,
    group: ListenerGroup,
    state: SharedState,
    local: ListenAddress,
    remote: SocketAddr,
) {
    let (client_io, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    let (mut sender, connection) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(connection);

    for request in 0..HEALTHY_REQUESTS {
        let (response, _) = sender
            .send_request(
                http::Request::builder()
                    .uri("http://localhost/")
                    .body(())
                    .unwrap(),
                true,
            )
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(3), response)
            .await
            .unwrap_or_else(|_| panic!("healthy client {client} request {request} timed out"))
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let mut output = Vec::new();
        while let Some(data) = body.data().await {
            let data = data.unwrap();
            body.flow_control().release_capacity(data.len()).unwrap();
            output.extend_from_slice(&data);
        }
        assert_eq!(output, b"healthy");
    }

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap_or_else(|_| panic!("healthy client {client} left its handler running"))
        .unwrap();
}

fn malformed_frame(case: u64, seed: &mut u64) -> Vec<u8> {
    let stream_id = odd_stream(*seed);
    match case % 8 {
        // DATA is never valid on stream zero.
        0 => frame(
            FRAME_DATA,
            next(seed) as u8,
            0,
            &random_payload(seed, 0, 16),
        ),
        // RST_STREAM is exactly four bytes.
        1 => {
            let length = (next(seed) % 4) as usize;
            frame(
                FRAME_RST_STREAM,
                0,
                stream_id,
                &random_payload(seed, length, length),
            )
        }
        // PING is exactly eight bytes.
        2 => {
            let mut length = (next(seed) % 17) as usize;
            if length == 8 {
                length = 7;
            }
            frame(
                FRAME_PING,
                (next(seed) as u8) & FLAG_ACK,
                0,
                &random_payload(seed, length, length),
            )
        }
        // A SETTINGS ACK must not carry a payload.
        3 => {
            let length = 1 + (next(seed) % 12) as usize;
            frame(
                FRAME_SETTINGS,
                FLAG_ACK,
                0,
                &random_payload(seed, length, length),
            )
        }
        // WINDOW_UPDATE increment zero is a connection error.
        4 => frame(FRAME_WINDOW_UPDATE, 0, 0, &0_u32.to_be_bytes()),
        // HPACK index 0 is invalid.
        5 => {
            let mut payload = vec![0x80];
            payload.extend(random_payload(seed, 0, 8));
            frame(
                FRAME_HEADERS,
                FLAG_END_HEADERS | ((next(seed) as u8) & FLAG_END_STREAM),
                stream_id,
                &payload,
            )
        }
        // Padding longer than the remaining payload is invalid.
        6 => {
            let pad = 1 + (next(seed) as u8);
            frame(
                FRAME_HEADERS,
                FLAG_PADDED | FLAG_END_HEADERS | FLAG_END_STREAM,
                stream_id,
                &[pad],
            )
        }
        // CONTINUATION without an open header block is invalid.
        _ => {
            let stream = if next(seed) & 1 == 0 { 0 } else { stream_id };
            frame(
                FRAME_CONTINUATION,
                FLAG_END_HEADERS,
                stream,
                &random_payload(seed, 0, 12),
            )
        }
    }
}

async fn wait_for_terminal<R>(client: &mut R)
where
    R: AsyncRead + Unpin,
{
    loop {
        let mut header = [0_u8; 9];
        match client.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::BrokenPipe
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                return;
            }
            Err(error) => panic!("failed to read HTTP/2 frame: {error}"),
        }
        let length =
            ((header[0] as usize) << 16) | ((header[1] as usize) << 8) | header[2] as usize;
        let mut payload = vec![0_u8; length];
        client.read_exact(&mut payload).await.unwrap();
        if header[3] == FRAME_GOAWAY {
            return;
        }
    }
}

fn next(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *seed;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn odd_stream(seed: u64) -> u32 {
    ((seed as u32) & 0x7fff_fffe) | 3
}

fn random_payload(seed: &mut u64, min: usize, max: usize) -> Vec<u8> {
    let length = min + (next(seed) as usize % (max - min + 1));
    (0..length).map(|_| next(seed) as u8).collect()
}
