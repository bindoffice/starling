use std::path::Path;

use super::support::*;
use super::*;
use crate::server::run::reload;

const LISTEN_PORT: u16 = 18920;
const BODY_SIZE: usize = 4 * 1024 * 1024;

#[tokio::test]
async fn slow_client_reset_cleans_a_buffered_response_spill_and_cancels_upstream() {
    let Some((upstream, backend)) = slow_spill_upstream().await else {
        return;
    };
    let (prefix, config) = test_config(&spill_config("proxy-temp", upstream, 1000), &[]);
    let state = test_state(&config);
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state);

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let temp_root = prefix.join("proxy-temp");
    wait_for_generated_temp_file(&temp_root, true).await;
    reset_client(client);

    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("client reset should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("client reset should cancel the stalled upstream response")
        .unwrap();
    wait_for_generated_temp_file(&temp_root, false).await;
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn reload_during_a_slow_spill_keeps_the_original_temp_root_until_cancel_cleanup() {
    let Some((upstream, backend)) = slow_spill_upstream().await else {
        return;
    };
    let (prefix, config) = test_config(&spill_config("temp-before-reload", upstream, 1000), &[]);
    let state = test_state(&config);
    let config_path = prefix.join("conf/nginx.conf");
    let frontend = bind_frontend().await;
    let frontend_address = frontend.local_addr().unwrap();
    let server = spawn_frontend(frontend, &config, state.clone());

    let mut client = TcpStream::connect(frontend_address).await.unwrap();
    socket2::SockRef::from(&client)
        .set_recv_buffer_size(1024)
        .unwrap();
    client
        .write_all(b"GET /spill HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let old_temp_root = prefix.join("temp-before-reload");
    wait_for_generated_temp_file(&old_temp_root, true).await;
    std::fs::write(
        &config_path,
        spill_config("temp-after-reload", upstream, 1001),
    )
    .unwrap();
    reload(&state, &config_path, &prefix).await;
    let snapshot = state.config_snapshot().await;
    assert_eq!(
        snapshot.config.http.as_ref().unwrap().servers[0]
            .context
            .client_max_body_size,
        1001
    );
    assert!(
        !temp_files(&old_temp_root).is_empty(),
        "reload must not consume or remove the in-flight request's spill"
    );

    reset_client(client);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .expect("client reset after reload should stop the connection handler")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), backend)
        .await
        .expect("client reset after reload should cancel the upstream")
        .unwrap();
    wait_for_generated_temp_file(&old_temp_root, false).await;
    assert!(
        temp_files(&prefix.join("temp-after-reload")).is_empty(),
        "the reloaded config must not receive stale files from the old request"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn slow_spill_upstream() -> Option<(SocketAddr, tokio::task::JoinHandle<()>)> {
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

async fn bind_frontend() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test frontend")
}

fn reset_client(client: TcpStream) {
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);
}

fn spawn_frontend(
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

async fn wait_for_generated_temp_file(root: &Path, present: bool) {
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

fn temp_files(root: &Path) -> Vec<std::path::PathBuf> {
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

fn spill_config(temp_path: &str, upstream: SocketAddr, marker: u64) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                client_max_body_size {marker};
                server {{
                    listen 127.0.0.1:{LISTEN_PORT};
                    location / {{
                        proxy_pass http://{upstream};
                        proxy_buffer_size 1k;
                        proxy_buffers 4 1k;
                        proxy_busy_buffers_size 2k;
                        proxy_temp_file_write_size 1k;
                        proxy_temp_path {temp_path};
                        proxy_max_temp_file_size 5m;
                        proxy_read_timeout 30s;
                    }}
                }}
            }}
        "#
    )
}
