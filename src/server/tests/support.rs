use super::*;
use crate::server::stream::handle_stream_accepted;

pub(super) fn run_with_worker_stack(
    future: impl std::future::Future<Output = ()> + Send + 'static,
) {
    std::thread::Builder::new()
        .stack_size(crate::runtime::WORKER_THREAD_STACK_SIZE)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future);
        })
        .unwrap()
        .join()
        .unwrap();
}

pub(super) async fn stream_response(config: &Arc<RuntimeConfig>, client_input: &[u8]) -> Vec<u8> {
    let route = build_stream_routes(config)[0].clone();
    let local = route.address.clone();
    let state = test_state(config);
    let transport_remote: SocketAddr = "127.0.0.1:45681".parse().unwrap();
    let (mut client, inbound) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_stream_accepted(
        inbound,
        ListenAddress::Inet(transport_remote),
        local,
        route,
        state,
    ));
    client.write_all(client_input).await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    response
}

pub(super) fn stream_client_hello(host: &str, alpn: &[&str]) -> Vec<u8> {
    let mut extensions = Vec::new();
    let mut sni = Vec::new();
    sni.extend_from_slice(&(host.len() as u16 + 3).to_be_bytes());
    sni.push(0);
    sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
    sni.extend_from_slice(host.as_bytes());
    extensions.extend_from_slice(&0_u16.to_be_bytes());
    extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
    extensions.extend_from_slice(&sni);

    let mut protocols = Vec::new();
    for protocol in alpn {
        protocols.push(protocol.len() as u8);
        protocols.extend_from_slice(protocol.as_bytes());
    }
    let mut alpn_data = Vec::new();
    alpn_data.extend_from_slice(&(protocols.len() as u16).to_be_bytes());
    alpn_data.extend_from_slice(&protocols);
    extensions.extend_from_slice(&16_u16.to_be_bytes());
    extensions.extend_from_slice(&(alpn_data.len() as u16).to_be_bytes());
    extensions.extend_from_slice(&alpn_data);

    extensions.extend_from_slice(&43_u16.to_be_bytes());
    extensions.extend_from_slice(&3_u16.to_be_bytes());
    extensions.extend_from_slice(&[2, 3, 4]);

    let mut body = Vec::new();
    body.extend_from_slice(&[3, 3]);
    body.extend_from_slice(&[0_u8; 32]);
    body.push(0);
    body.extend_from_slice(&[0, 2, 0x13, 0x01]);
    body.extend_from_slice(&[1, 0]);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);

    let mut handshake = vec![
        1,
        (body.len() >> 16) as u8,
        (body.len() >> 8) as u8,
        body.len() as u8,
    ];
    handshake.extend_from_slice(&body);

    let mut record = vec![0x16, 3, 1];
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

pub(super) async fn request_body(config: &Arc<RuntimeConfig>, request: &str) -> String {
    String::from_utf8(request_bytes(config, request).await).unwrap()
}

pub(super) async fn request_bytes(config: &Arc<RuntimeConfig>, request: &str) -> Vec<u8> {
    request_bytes_with_state(config, test_state(config), request).await
}

pub(super) fn split_http_response(response: &[u8]) -> (&str, &[u8]) {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response headers");
    (
        std::str::from_utf8(&response[..header_end]).expect("UTF-8 response headers"),
        &response[header_end + 4..],
    )
}

pub(super) async fn request_body_with_state(
    config: &Arc<RuntimeConfig>,
    state: SharedState,
    request: &str,
) -> String {
    String::from_utf8(request_bytes_with_state(config, state, request).await).unwrap()
}

pub(super) async fn request_bytes_with_state(
    config: &Arc<RuntimeConfig>,
    state: SharedState,
    request: &str,
) -> Vec<u8> {
    let group = build_listener_groups(config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(async move {
        handle_io(
            server,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
        )
        .await;
    });
    client.write_all(request.as_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    task.await.unwrap();
    response
}

pub(super) fn test_state(config: &Arc<RuntimeConfig>) -> SharedState {
    let logger =
        Arc::new(Logger::new(&config.core.error_logs, &config.prefix).expect("error logger"));
    let http_error_loggers = build_http_error_loggers(config).expect("HTTP error loggers");
    let stream_loggers = build_stream_loggers(config).expect("stream error loggers");
    let (reload, _) = watch::channel(0_u64);
    SharedState {
        config: Arc::new(ArcSwap::from_pointee(RuntimeSnapshot {
            config: config.clone(),
            listener_groups: build_listener_groups(config).into(),
            http_error_loggers,
            upstream_keepalive_generation: 0,
        })),
        reload: Arc::new(reload),
        tls_configs: Arc::new(RwLock::new(HashMap::new())),
        stream_tls_configs: Arc::new(RwLock::new(HashMap::new())),
        logger,
        stream_loggers: Arc::new(RwLock::new(stream_loggers)),
        limits: Arc::new(Mutex::new(LimitState::default())),
        stats: Arc::new(ConnectionStats::default()),
        upstream_keepalive: Arc::new(UpstreamKeepalivePool::default()),
        proxy_cache: Arc::new(ProxyCacheRegistry::new(config.http.as_ref())),
        client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
        http2_budget: Arc::new(Http2Budget::new()),
        auto_https: Arc::new(AutoHttpsState::new(config.auto_https.clone())),
    }
}

pub(super) fn read_fastcgi_test_length(data: &[u8], position: &mut usize) -> usize {
    let first = data[*position];
    if first & 0x80 == 0 {
        *position += 1;
        return first as usize;
    }
    let length = u32::from_be_bytes([
        first & 0x7f,
        data[*position + 1],
        data[*position + 2],
        data[*position + 3],
    ]) as usize;
    *position += 4;
    length
}

pub(super) async fn read_fastcgi_test_record<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> (u8, Vec<u8>) {
    let mut header = [0_u8; 8];
    reader.read_exact(&mut header).await.unwrap();
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    let padding = header[6] as usize;
    let mut content = vec![0_u8; length];
    reader.read_exact(&mut content).await.unwrap();
    if padding > 0 {
        let mut padding = vec![0_u8; padding];
        reader.read_exact(&mut padding).await.unwrap();
    }
    (header[1], content)
}

pub(super) fn test_socket_path(label: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("starling-{label}-{}-{id}.sock", std::process::id()))
}

pub(super) fn contains_generated_temp_file(path: &std::path::Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        path.is_dir() && contains_generated_temp_file(&path)
            || path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".starling-tmp-"))
    })
}

pub(super) fn test_unix_listener(path: &std::path::Path) -> Option<tokio::net::UnixListener> {
    match tokio::net::UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping UNIX socket test: binding {} is not permitted",
                path.display()
            );
            None
        }
        Err(error) => panic!("bind {}: {error}", path.display()),
    }
}

pub(super) fn parse_null_separated_params(data: &[u8]) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut fields = data.split(|byte| *byte == 0);
    while let (Some(name), Some(value)) = (fields.next(), fields.next()) {
        if name.is_empty() && value.is_empty() {
            break;
        }
        values.insert(
            String::from_utf8(name.to_vec()).unwrap(),
            String::from_utf8(value.to_vec()).unwrap(),
        );
    }
    values
}

pub(super) fn parse_fastcgi_test_params(data: &[u8]) -> HashMap<String, String> {
    let mut values = HashMap::new();
    let mut position = 0;
    while position < data.len() {
        let name_length = read_fastcgi_test_length(data, &mut position);
        let value_length = read_fastcgi_test_length(data, &mut position);
        let name = String::from_utf8(data[position..position + name_length].to_vec()).unwrap();
        position += name_length;
        let value = String::from_utf8(data[position..position + value_length].to_vec()).unwrap();
        position += value_length;
        values.insert(name, value);
    }
    values
}

pub(super) fn test_config(source: &str, files: &[(&str, &str)]) -> (PathBuf, Arc<RuntimeConfig>) {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let prefix = std::env::temp_dir().join(format!("starling-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(prefix.join("conf")).unwrap();
    std::fs::create_dir_all(prefix.join("logs")).unwrap();
    for (path, contents) in files {
        let path = prefix.join(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }
    let config_path = prefix.join("conf/nginx.conf");
    std::fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .expect("parse configuration");
    let config = compile(document, prefix.clone(), config_path).expect("compile configuration");
    (prefix, Arc::new(config))
}

pub(super) fn test_config_error(source: &str) -> String {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let prefix = std::env::temp_dir().join(format!("starling-test-{}-{id}", std::process::id()));
    std::fs::create_dir_all(prefix.join("conf")).unwrap();
    std::fs::create_dir_all(prefix.join("logs")).unwrap();
    let config_path = prefix.join("conf/nginx.conf");
    std::fs::write(&config_path, source).unwrap();
    let document = ConfigLoader::new(prefix.clone())
        .load(&config_path)
        .expect("parse configuration");
    let error = compile(document, prefix.clone(), config_path)
        .expect_err("configuration should fail")
        .to_string();
    let _ = std::fs::remove_dir_all(prefix);
    error
}

pub(super) fn upstream_config(body: &str) -> String {
    format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream backend {{
                        {body}
                    }}
                    server {{
                        listen 127.0.0.1:18096;
                        location / {{ proxy_pass http://backend; }}
                    }}
                }}
            "#
    )
}

pub(super) fn location_config(directives: &str) -> String {
    format!(
        r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18097;
                        {directives}
                        location / {{ return 200 "ok\n"; }}
                    }}
                }}
            "#
    )
}
