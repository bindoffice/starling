use std::path::Path;

use super::*;

pub(super) async fn slow_spill_upstream() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping P0 spill failure test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind spill upstream: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {BODY_SIZE}\r\nConnection: close\r\n\r\n{}",
            "x".repeat(BODY_SIZE)
        );
        if let Err(error) = stream.write_all(response.as_bytes()).await {
            assert_eq!(
                error.kind(),
                io::ErrorKind::ConnectionReset,
                "unexpected upstream write failure: {error}"
            );
            return;
        }
        stream.flush().await.unwrap();
        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("client reset should close the stalled upstream");
        assert_eq!(closed.unwrap(), 0, "upstream was not closed after reset");
    });
    Some((address, backend))
}

pub(super) async fn trickle_spill_upstream(
    chunk_delay: Duration,
) -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping P0 slow-upstream test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind slow spill upstream: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {SLOW_BODY_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let chunk = vec![b'x'; 32 * 1024];
        for _ in 0..SLOW_BODY_SIZE / chunk.len() {
            stream.write_all(&chunk).await.unwrap();
            tokio::time::sleep(chunk_delay).await;
        }
        stream.flush().await.unwrap();
        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("completed slow upstream should be closed after the response");
        assert_eq!(closed.unwrap(), 0);
    });
    Some((address, backend))
}

pub(super) async fn cancellable_trickle_spill_upstream()
-> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping P0 slow-upstream cancellation test: TCP binding is not permitted");
            return None;
        }
        Err(error) => panic!("bind cancellable slow spill upstream: {error}"),
    };
    let address = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                     Content-Length: {SLOW_BODY_SIZE}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let chunk = vec![b'x'; 32 * 1024];
        let mut sent = 0;
        while sent < SLOW_BODY_SIZE {
            match stream.write_all(&chunk).await {
                Ok(()) => {
                    sent += chunk.len();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::BrokenPipe
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::ConnectionReset
                    ) =>
                {
                    return;
                }
                Err(error) => panic!("unexpected slow upstream write failure: {error}"),
            }
        }
        let mut byte = [0_u8; 1];
        match stream.read(&mut byte).await {
            Ok(0) => {}
            Ok(read) => panic!("unexpected upstream data after response body: {read}"),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                ) => {}
            Err(error) => panic!("unexpected slow upstream read failure: {error}"),
        }
    });
    Some((address, backend))
}

pub(super) async fn bind_frontend() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test frontend")
}

pub(super) fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
}

pub(super) fn spawn_frontend(
    listener: TcpListener,
    config: &Arc<RuntimeConfig>,
    state: SharedState,
) -> tokio::task::JoinHandle<()> {
    let group = build_listener_groups(config)[0].clone();
    tokio::spawn(async move {
        let (stream, remote) = listener.accept().await.unwrap();
        let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
        let local = stream
            .local_addr()
            .map(ListenAddress::Inet)
            .unwrap_or_else(|_| group.address.clone());
        handle_io_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
            None,
            control,
        )
        .await;
    })
}

async fn read_request_head(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream closed before request headers");
        request.extend_from_slice(&buffer[..read]);
    }
}

pub(super) async fn read_response_with_body(
    stream: &mut TcpStream,
    expected_body: usize,
) -> Result<Vec<u8>, Vec<u8>> {
    let mut response = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let mut buffer = [0_u8; 8192];
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out after {} response bytes: {}",
            response.len(),
            String::from_utf8_lossy(&response[..response.len().min(256)])
        );
        let Ok(read) = tokio::time::timeout(remaining, stream.read(&mut buffer)).await else {
            return Err(response);
        };
        let read = read.unwrap();
        assert!(
            read > 0,
            "connection closed after {} response bytes",
            response.len()
        );
        response.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n")
            && response.len() >= header_end + 4 + expected_body
        {
            return Ok(response);
        }
    }
}

pub(super) async fn wait_for_generated_temp_file(root: &Path, present: bool) {
    for _ in 0..200 {
        if !temp_files(root).is_empty() == present {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for generated temp files under {} (present={present}): {:?}",
        root.display(),
        temp_files(root)
    );
}

pub(super) fn temp_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(temp_files(&path));
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".starling-tmp-"))
        {
            files.push(path);
        }
    }
    files
}

pub(super) fn spill_config(temp_path: &str, upstream: SocketAddr, marker: u64) -> String {
    spill_config_with_timeout(temp_path, upstream, marker, "30s")
}

pub(super) fn spill_config_with_timeout(
    temp_path: &str,
    upstream: SocketAddr,
    marker: u64,
    read_timeout: &str,
) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_max_body_size {marker};
                server {{
                    listen 127.0.0.1:{LISTEN_PORT};
                    lingering_close off;
                    location / {{
                        proxy_pass http://{upstream};
                        proxy_buffer_size 1k;
                        proxy_buffers 4 1k;
                        proxy_busy_buffers_size 2k;
                        proxy_temp_file_write_size 1k;
                        proxy_temp_path {temp_path};
                        proxy_max_temp_file_size 5m;
                        proxy_read_timeout {read_timeout};
                    }}
                }}
            }}
        "#
    )
}
