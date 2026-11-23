use super::*;

const HEALTHY_CLIENTS: u64 = 4;
const REQUESTS_PER_CLIENT: usize = 32;
const MALFORMED_WORKERS: u64 = 8;
const DEFAULT_SOAK_CASES: u64 = 1024;

const CORPUS: &[(&str, &str)] = &[
    (
        "settings-stream-id",
        include_str!("../../../../../tests/fixtures/http2-corpus/settings-stream-id.hex"),
    ),
    (
        "rst-short",
        include_str!("../../../../../tests/fixtures/http2-corpus/rst-short.hex"),
    ),
    (
        "ping-short",
        include_str!("../../../../../tests/fixtures/http2-corpus/ping-short.hex"),
    ),
    (
        "window-update-zero",
        include_str!("../../../../../tests/fixtures/http2-corpus/window-update-zero.hex"),
    ),
    (
        "orphan-continuation",
        include_str!("../../../../../tests/fixtures/http2-corpus/orphan-continuation.hex"),
    ),
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_corpus_cases_do_not_disrupt_a_healthy_connection() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 128; }
            http {
                server {
                    listen 127.0.0.1:18464 http2;
                    server_name localhost;
                    location / { return 200 "healthy"; }
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();

    let (healthy_io, healthy_server_io) = tokio::io::duplex(64 * 1024);
    let healthy_server = tokio::spawn(handle_http_connection(
        healthy_server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local.clone()),
        group.clone(),
        state.clone(),
    ));
    let (mut sender, connection) = h2::client::handshake(healthy_io).await.unwrap();
    let driver = tokio::spawn(connection);

    for (name, source) in CORPUS {
        let frames = parse_hex_fixture(name, source);
        assert_goaway_reason(
            name,
            frames,
            h2::Reason::PROTOCOL_ERROR,
            group.clone(),
            state.clone(),
            local.clone(),
            remote,
        )
        .await;
        assert_healthy_response(&mut sender, name).await;
    }

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(3), healthy_server)
        .await
        .expect("healthy corpus client left its handler running")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mutation_soak_keeps_healthy_http2_traffic_available() {
    let cases = std::env::var("STARLING_HTTP2_SOAK_CASES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SOAK_CASES)
        .max(MALFORMED_WORKERS);
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 256; }
            http {
                server {
                    listen 127.0.0.1:18464 http2;
                    server_name localhost;
                    location / { return 200 "healthy"; }
                }
            }
        "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
    let workers = MALFORMED_WORKERS + HEALTHY_CLIENTS;
    let start = std::sync::Arc::new(tokio::sync::Barrier::new(workers as usize));
    let mut tasks = tokio::task::JoinSet::new();

    for worker in 0..MALFORMED_WORKERS {
        let group = group.clone();
        let state = state.clone();
        let local = local.clone();
        let start = start.clone();
        tasks.spawn(async move {
            start.wait().await;
            let mut case = worker;
            while case < cases {
                run_malformed_connection(case, group.clone(), state.clone(), local.clone(), remote)
                    .await;
                case += MALFORMED_WORKERS;
            }
        });
    }

    for client in 0..HEALTHY_CLIENTS {
        let group = group.clone();
        let state = state.clone();
        let local = local.clone();
        let start = start.clone();
        tasks.spawn(async move {
            start.wait().await;
            run_healthy_connection(client, group, state, local, remote, REQUESTS_PER_CLIENT).await;
        });
    }

    while let Some(result) = tasks.join_next().await {
        result.unwrap_or_else(|error| panic!("HTTP/2 mutation soak task panicked: {error}"));
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
    let mut seed = 0xd1b5_4a32_d192_ed03_u64 ^ case;
    let malformed = super::mutations::malformed_frame(case, &mut seed);
    let mut frames = Vec::new();
    if case.is_multiple_of(3) {
        frames.extend(frame(
            FRAME_HEADERS,
            FLAG_END_HEADERS,
            1,
            b"\x83\x86\x84\x01\x09localhost",
        ));
    }
    frames.extend(malformed);

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
    client.write_all(&frames).await.unwrap();
    client.flush().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(3),
        super::mutations::wait_for_terminal(&mut client),
    )
    .await
    .unwrap_or_else(|_| panic!("soak mutation case {case} did not terminate"));
    drop(client);
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap_or_else(|_| panic!("soak mutation case {case} left its handler running"))
        .unwrap_or_else(|error| panic!("soak mutation case {case} panicked: {error}"));
}

async fn run_healthy_connection(
    client: u64,
    group: ListenerGroup,
    state: SharedState,
    local: ListenAddress,
    remote: SocketAddr,
    requests: usize,
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
    for request in 0..requests {
        assert_healthy_response(
            &mut sender,
            &format!("soak client {client} request {request}"),
        )
        .await;
    }
    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap_or_else(|_| panic!("healthy soak client {client} left its handler running"))
        .unwrap();
}

async fn assert_healthy_response(sender: &mut h2::client::SendRequest<Bytes>, label: &str) {
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
        .unwrap_or_else(|_| panic!("healthy request timed out after {label}"))
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "after {label}");
    let mut body = response.into_body();
    let mut output = Vec::new();
    while let Some(data) = body.data().await {
        let data = data.unwrap();
        body.flow_control().release_capacity(data.len()).unwrap();
        output.extend_from_slice(&data);
    }
    assert_eq!(output, b"healthy", "after {label}");
}

async fn assert_goaway_reason(
    name: &str,
    frames: Vec<u8>,
    expected: h2::Reason,
    group: ListenerGroup,
    state: SharedState,
    local: ListenAddress,
    remote: SocketAddr,
) {
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
    client
        .write_all(&frame(FRAME_SETTINGS, 0, 0, &[]))
        .await
        .unwrap();
    client.write_all(&frames).await.unwrap();
    client.flush().await.unwrap();

    let reason = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let (kind, _flags, stream_id, payload) = read_frame(&mut client).await;
            if kind == FRAME_GOAWAY {
                assert_eq!(stream_id, 0, "corpus case {name} sent stream-scoped GOAWAY");
                assert!(payload.len() >= 8, "corpus case {name} truncated GOAWAY");
                return u32::from_be_bytes(payload[4..8].try_into().unwrap());
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("corpus case {name} did not send GOAWAY"));
    assert_eq!(reason, u32::from(expected), "corpus case {name}");

    drop(client);
    tokio::time::timeout(Duration::from_secs(3), server_task)
        .await
        .unwrap_or_else(|_| panic!("corpus case {name} left its handler running"))
        .unwrap();
}

fn parse_hex_fixture(name: &str, source: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (line_number, line) in source.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }
        for token in line.split_whitespace() {
            let byte = u8::from_str_radix(token, 16).unwrap_or_else(|_| {
                panic!(
                    "corpus fixture {name}:{} contains invalid hex {token:?}",
                    line_number + 1
                )
            });
            bytes.push(byte);
        }
    }
    assert!(!bytes.is_empty(), "corpus fixture {name} is empty");
    bytes
}
