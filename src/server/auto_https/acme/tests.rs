use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct FakeAcmeServer {
    challenge_seen: AtomicBool,
    finalized: AtomicBool,
}

#[derive(Default)]
struct FakeDnsAcmeServer {
    challenges_seen: AtomicUsize,
    finalized: AtomicBool,
}

#[test]
fn parses_numeric_and_http_date_retry_after_values() {
    assert_eq!(parse_retry_after("3"), Some(3));
    assert_eq!(parse_retry_after("not-a-delay"), None);

    let retry_at = SystemTime::now() + Duration::from_secs(30);
    let seconds = parse_retry_after(&httpdate::fmt_http_date(retry_at)).unwrap();
    assert!((29..=30).contains(&seconds), "{seconds}");
}

#[test]
fn constructs_dns01_record_names_and_values() {
    let challenge =
        Dns01Challenge::from_authorization("Example.COM.", false, "test-token.thumbprint").unwrap();
    assert_eq!(challenge.domain, "example.com");
    assert_eq!(challenge.record_name, "_acme-challenge.example.com");
    assert_eq!(
        challenge.record_value,
        "kUHUw4hPF1COEN2DPVPBz1XhZWRJKqOmcJGXObqUzzY"
    );

    let wildcard =
        Dns01Challenge::from_authorization("example.com", true, "test-token.thumbprint").unwrap();
    assert_eq!(wildcard.domain, "*.example.com");
    assert_eq!(wildcard.record_name, challenge.record_name);
    assert_eq!(wildcard.record_value, challenge.record_value);

    let second =
        Dns01Challenge::from_authorization("example.com", false, "other-token.thumbprint").unwrap();
    assert_eq!(second.record_name, challenge.record_name);
    assert_ne!(second.record_value, challenge.record_value);
}

#[cfg(unix)]
#[tokio::test]
async fn dns01_hook_receives_environment_and_json_payload() {
    use std::os::unix::fs::PermissionsExt;

    let directory = std::env::temp_dir().join(format!(
        "starling-dns-hook-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let hook = directory.join("hook.sh");
    let log = directory.join("hook.log");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n{{\n\
             printf 'ENV|%s|%s|%s|%s\\n' \
             \"$STARLING_ACME_DNS_ACTION\" \"$STARLING_ACME_DNS_NAME\" \
             \"$STARLING_ACME_DNS_VALUE\" \"$STARLING_ACME_DNS_DOMAIN\"\n\
             cat\n\
             printf '\\n'\n\
             }} >> '{}'\n",
            log.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();
    let challenge =
        Dns01Challenge::from_authorization("example.com", true, "test-token.thumbprint").unwrap();

    run_dns_hook(&hook, DnsHookAction::Present, &challenge)
        .await
        .unwrap();
    run_dns_hook(&hook, DnsHookAction::Cleanup, &challenge)
        .await
        .unwrap();

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("ENV|present|_acme-challenge.example.com|"));
    assert!(log.contains("|*.example.com\n"));
    assert!(log.contains("ENV|cleanup|_acme-challenge.example.com|"));
    assert!(log.contains(&format!(
        "\"action\":\"present\",\"domain\":\"*.example.com\",\"name\":\"{}\",\"value\":\"{}\"",
        challenge.record_name, challenge.record_value
    )));
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn completes_http01_order_and_persists_certificate() {
    let directory = std::env::temp_dir().join(format!(
        "starling-acme-order-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping ACME order test: binding a local TCP socket is not permitted");
            let _ = std::fs::remove_dir_all(directory);
            return;
        }
        Err(error) => panic!("bind fake ACME server: {error}"),
    };
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = Arc::new(FakeAcmeServer::default());
    let state = Arc::new(AutoHttpsState::new(AutoHttpsConfig::new(&directory)));
    let server_state = state.clone();
    let server_base_url = base_url.clone();
    let server_task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            handle_fake_request(stream, &server_base_url, &server, &server_state).await;
        }
    });

    let domains = vec!["public.starling.dev".to_string()];
    let order = AcmeOrder {
        domains: domains.clone(),
        paths: certificate_paths(&directory, &domains),
        staging: false,
        directory_url: format!("{base_url}/directory"),
        ca_file: None,
        email: Some("admin@example.com".to_string()),
        challenge: AutoHttpsChallenge::Http01,
        dns_hook: None,
        dns_propagation_delay: Duration::from_secs(30),
    };
    obtain_certificate(&order, &state, None).await.unwrap();
    server_task.abort();

    assert_eq!(order.paths.source(), Some(CertificateSource::Acme));
    assert!(order.paths.cert.exists());
    assert!(order.paths.key.exists());
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[tokio::test]
async fn completes_dns01_order_with_shared_wildcard_record() {
    use std::os::unix::fs::PermissionsExt;

    let directory = std::env::temp_dir().join(format!(
        "starling-acme-dns-order-{}-{}",
        std::process::id(),
        AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping DNS-01 ACME order test: binding a local TCP socket is not permitted"
            );
            let _ = std::fs::remove_dir_all(directory);
            return;
        }
        Err(error) => panic!("bind fake DNS-01 ACME server: {error}"),
    };
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = Arc::new(FakeDnsAcmeServer::default());
    let state_dir = directory.join("dns-state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let hook_log = directory.join("hook.log");
    let hook = directory.join("dns-hook.sh");
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\n\
             printf '%s|%s|%s\\n' \"$STARLING_ACME_DNS_ACTION\" \
             \"$STARLING_ACME_DNS_NAME\" \"$STARLING_ACME_DNS_VALUE\" >> '{}'\n\
             case \"$STARLING_ACME_DNS_ACTION\" in\n\
               present) : > '{}/'\"$STARLING_ACME_DNS_VALUE\" ;;\n\
               cleanup) rm -f '{}/'\"$STARLING_ACME_DNS_VALUE\" ;;\n\
             esac\n",
            hook_log.display(),
            state_dir.display(),
            state_dir.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&hook).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&hook, permissions).unwrap();
    let server_state_dir = state_dir.clone();
    let server_base_url = base_url.clone();
    let server_task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            handle_fake_dns_request(stream, &server_base_url, &server, &server_state_dir).await;
        }
    });

    let domains = vec!["example.com".to_string(), "*.example.com".to_string()];
    let order = AcmeOrder {
        domains: domains.clone(),
        paths: certificate_paths(&directory, &domains),
        staging: false,
        directory_url: format!("{base_url}/directory"),
        ca_file: None,
        email: None,
        challenge: AutoHttpsChallenge::Dns01,
        dns_hook: Some(hook),
        dns_propagation_delay: Duration::ZERO,
    };
    obtain_certificate(
        &order,
        &AutoHttpsState::new(AutoHttpsConfig::new(&directory)),
        None,
    )
    .await
    .unwrap();
    server_task.abort();

    assert_eq!(
        std::fs::read_dir(&state_dir).unwrap().count(),
        0,
        "all DNS-01 records should be cleaned up"
    );
    let log = std::fs::read_to_string(&hook_log).unwrap();
    assert_eq!(
        log.matches("present|_acme-challenge.example.com|").count(),
        2
    );
    assert_eq!(
        log.matches("cleanup|_acme-challenge.example.com|").count(),
        2
    );
    assert_eq!(order.paths.source(), Some(CertificateSource::Acme));
    let _ = std::fs::remove_dir_all(directory);
}

async fn handle_fake_request(
    mut stream: TcpStream,
    base_url: &str,
    server: &FakeAcmeServer,
    state: &AutoHttpsState,
) {
    let (method, path) = read_request(&mut stream).await;
    let path = path.split('?').next().unwrap_or(&path);
    match (method.as_str(), path) {
        ("GET", "/directory") => {
            let body = json!({
                "newNonce": format!("{base_url}/nonce"),
                "newAccount": format!("{base_url}/account"),
                "newOrder": format!("{base_url}/order"),
            });
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("HEAD", "/nonce") => {
            write_response(&mut stream, 200, &[], "application/json", &[]).await;
        }
        ("POST", "/account") => {
            write_json_response(
                &mut stream,
                201,
                &json!({"status": "valid"}),
                &[("Location", format!("{base_url}/account/1"))],
            )
            .await;
        }
        ("POST", "/order") => {
            write_json_response(
                &mut stream,
                201,
                &json!({
                    "status": "pending",
                    "authorizations": [format!("{base_url}/authorization/1")],
                    "finalize": format!("{base_url}/finalize/1"),
                }),
                &[("Location", format!("{base_url}/order/1"))],
            )
            .await;
        }
        ("POST", "/authorization/1") => {
            let body = if server.challenge_seen.load(Ordering::SeqCst) {
                json!({"status": "valid"})
            } else {
                json!({
                    "status": "pending",
                    "challenges": [{
                        "type": "http-01",
                        "url": format!("{base_url}/challenge/1"),
                        "token": "test-token",
                    }],
                })
            };
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("POST", "/challenge/1") => {
            let authorization = state
                .challenge_response("test-token")
                .expect("HTTP-01 challenge should be published before validation");
            assert!(authorization.starts_with("test-token."));
            server.challenge_seen.store(true, Ordering::SeqCst);
            write_json_response(&mut stream, 200, &json!({"status": "processing"}), &[]).await;
        }
        ("POST", "/finalize/1") => {
            server.finalized.store(true, Ordering::SeqCst);
            write_json_response(&mut stream, 200, &json!({"status": "processing"}), &[]).await;
        }
        ("POST", "/order/1") => {
            let body = if server.finalized.load(Ordering::SeqCst) {
                json!({
                    "status": "valid",
                    "certificate": format!("{base_url}/certificate/1"),
                })
            } else if server.challenge_seen.load(Ordering::SeqCst) {
                json!({"status": "ready"})
            } else {
                json!({"status": "pending"})
            };
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("POST", "/certificate/1") => {
            let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            let certificate = std::fs::read(fixture.join("test.crt")).unwrap();
            write_response(
                &mut stream,
                200,
                &certificate,
                "application/pem-certificate-chain",
                &[],
            )
            .await;
        }
        _ => write_json_response(&mut stream, 404, &json!({"error": "not found"}), &[]).await,
    }
}

async fn handle_fake_dns_request(
    mut stream: TcpStream,
    base_url: &str,
    server: &FakeDnsAcmeServer,
    state_dir: &Path,
) {
    let (method, path) = read_request(&mut stream).await;
    let path = path.split('?').next().unwrap_or(&path);
    match (method.as_str(), path) {
        ("GET", "/directory") => {
            let body = json!({
                "newNonce": format!("{base_url}/nonce"),
                "newAccount": format!("{base_url}/account"),
                "newOrder": format!("{base_url}/order"),
            });
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("HEAD", "/nonce") => {
            write_response(&mut stream, 200, &[], "application/json", &[]).await;
        }
        ("POST", "/account") => {
            write_json_response(
                &mut stream,
                201,
                &json!({"status": "valid"}),
                &[("Location", format!("{base_url}/account/1"))],
            )
            .await;
        }
        ("POST", "/order") => {
            write_json_response(
                &mut stream,
                201,
                &json!({
                    "status": "pending",
                    "authorizations": [
                        format!("{base_url}/authorization/1"),
                        format!("{base_url}/authorization/2"),
                    ],
                    "finalize": format!("{base_url}/finalize/1"),
                }),
                &[("Location", format!("{base_url}/order/1"))],
            )
            .await;
        }
        ("POST", "/authorization/1") => {
            let body = authorization_body(base_url, server, 1, false, "token-base");
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("POST", "/authorization/2") => {
            let body = authorization_body(base_url, server, 2, true, "token-wildcard");
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("POST", "/challenge/1") | ("POST", "/challenge/2") => {
            let present_records = std::fs::read_dir(state_dir).unwrap().count();
            assert_eq!(
                present_records, 2,
                "both DNS-01 TXT values must be present before validation starts"
            );
            server.challenges_seen.fetch_add(1, Ordering::SeqCst);
            write_json_response(&mut stream, 200, &json!({"status": "processing"}), &[]).await;
        }
        ("POST", "/finalize/1") => {
            server.finalized.store(true, Ordering::SeqCst);
            write_json_response(&mut stream, 200, &json!({"status": "processing"}), &[]).await;
        }
        ("POST", "/order/1") => {
            let body = if server.finalized.load(Ordering::SeqCst) {
                json!({
                    "status": "valid",
                    "certificate": format!("{base_url}/certificate/1"),
                })
            } else if server.challenges_seen.load(Ordering::SeqCst) == 2 {
                json!({"status": "ready"})
            } else {
                json!({"status": "pending"})
            };
            write_json_response(&mut stream, 200, &body, &[]).await;
        }
        ("POST", "/certificate/1") => {
            let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            let certificate = std::fs::read(fixture.join("test.crt")).unwrap();
            write_response(
                &mut stream,
                200,
                &certificate,
                "application/pem-certificate-chain",
                &[],
            )
            .await;
        }
        _ => write_json_response(&mut stream, 404, &json!({"error": "not found"}), &[]).await,
    }
}

fn authorization_body(
    base_url: &str,
    server: &FakeDnsAcmeServer,
    index: usize,
    wildcard: bool,
    token: &str,
) -> Value {
    if server.challenges_seen.load(Ordering::SeqCst) > 0 {
        return json!({"status": "valid"});
    }
    json!({
        "status": "pending",
        "identifier": {"type": "dns", "value": "example.com"},
        "wildcard": wildcard,
        "challenges": [{
            "type": "dns-01",
            "url": format!("{base_url}/challenge/{index}"),
            "token": token,
        }],
    })
}

async fn read_request(stream: &mut TcpStream) -> (String, String) {
    let mut data = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "client closed before sending request headers");
        data.extend_from_slice(&buffer[..read]);
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
    };
    let head = std::str::from_utf8(&data[..header_end]).unwrap();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap();
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap().to_string();
    let path = request_parts.next().unwrap().to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while data.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "client closed before sending request body");
        data.extend_from_slice(&buffer[..read]);
    }
    (method, path)
}

async fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    body: &Value,
    headers: &[(&str, String)],
) {
    let body = serde_json::to_vec(body).unwrap();
    write_response(stream, status, &body, "application/json", headers).await;
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    body: &[u8],
    content_type: &str,
    headers: &[(&str, String)],
) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        404 => "Not Found",
        _ => "Response",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n\
         Content-Length: {}\r\nReplay-Nonce: test-nonce\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    stream.shutdown().await.unwrap();
}
