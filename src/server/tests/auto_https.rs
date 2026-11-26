use super::support::*;
use super::*;
use crate::server::auto_https::{AutoHttpsState, prepare_auto_https};

#[test]
fn automatic_https_is_enabled_by_default() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18080;
                    server_name localhost;
                }
            }
        "#,
        &[],
    );
    assert!(config.auto_https.enabled);
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn unused_automatic_https_does_not_create_a_certificate_directory() {
    let (prefix, config) = test_config(
        r#"
            events {}
            http {
                server {
                    listen 127.0.0.1:18087;
                    server_name localhost;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    assert!(!prepared.auto_https.cert_dir.exists());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn local_https_adds_redirect_challenge_and_certificate() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18080;
            auto_https_https_port 18443;
            events {}
            http {
                server {
                    listen 127.0.0.1:18080;
                    server_name localhost dev.local;
                    location / { return 200 "normal\n"; }
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let prepared = Arc::new(prepared);
    let state = test_state(&prepared);
    let groups = build_listener_groups(&prepared);
    let http_group = groups
        .iter()
        .find(|group| group.address.port() == 18080)
        .unwrap()
        .clone();
    let https_group = groups
        .iter()
        .find(|group| group.address.port() == 18443)
        .unwrap()
        .clone();
    assert!(https_group.ssl);
    assert!(
        prepared.http.as_ref().unwrap().servers[0]
            .context
            .ssl_certificate
            .as_ref()
            .is_some_and(|path| path.exists())
    );
    assert!(prepared.auto_https.cert_dir.join("local-ca.crt").exists());

    let response = request_for_group(
        &prepared,
        state.clone(),
        http_group.clone(),
        "GET /path?x=1 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 308 "), "{response}");
    assert!(
        response.contains("Location: https://localhost:18443/path?x=1\r\n"),
        "{response}"
    );

    state
        .auto_https
        .set_challenge("token-123", "token-123.thumbprint".into());
    let response = request_for_group(
        &prepared,
        state,
        http_group,
        "GET /.well-known/acme-challenge/token-123 HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(response.ends_with("token-123.thumbprint"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn automatic_https_off_keeps_plain_http_behavior() {
    let (prefix, config) = test_config(
        r#"
            auto_https off;
            events {}
            http {
                server {
                    listen 127.0.0.1:18081;
                    server_name localhost;
                    location / { return 200 "plain\n"; }
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    let preparation = prepare_auto_https(&mut prepared, &auto_https).unwrap();
    assert!(preparation.notices.is_empty());
    let groups = build_listener_groups(&prepared);
    assert_eq!(groups.len(), 1);
    let state = test_state(&Arc::new(prepared));
    let response = request_for_group(
        &state.current_config(),
        state.clone(),
        groups[0].clone(),
        "GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(String::from_utf8(response).unwrap().ends_with("plain\n"));
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn automatic_https_redirect_does_not_bypass_invalid_host_rejection() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18086;
            auto_https_https_port 18448;
            events {}
            http {
                server {
                    listen 127.0.0.1:18086;
                    server_name localhost;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let prepared = Arc::new(prepared);
    let state = test_state(&prepared);
    let group = build_listener_groups(&prepared)
        .into_iter()
        .find(|group| group.address.port() == 18086)
        .unwrap();
    let response = request_for_group(
        &prepared,
        state,
        group,
        "GET / HTTP/1.1\r\nHost: bad host\r\nConnection: close\r\n\r\n",
    )
    .await;
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn local_certificate_completes_tls_handshake_with_sni() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18082;
            auto_https_https_port 18444;
            events {}
            http {
                server {
                    listen 127.0.0.1:18082;
                    server_name localhost;
                    location / { return 200 "secure:$scheme:$https:$ssl_server_name\n"; }
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let prepared = Arc::new(prepared);
    let group = build_listener_groups(&prepared)
        .into_iter()
        .find(|group| group.address.port() == 18444)
        .unwrap();
    assert!(group.ssl);
    let tls_config = build_tls_configs(&prepared, std::slice::from_ref(&group))
        .unwrap()
        .remove(&group.address)
        .unwrap();
    let state = test_state(&prepared);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45683".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(async move {
        let stream = accept_http_tls(server_io, tls_config).await.unwrap();
        let mut connection = ConnectionAddresses::direct(ListenAddress::Inet(remote), local);
        connection.tls = Some(tls_connection_info(stream.get_ref().1));
        handle_io(stream, connection, group, state).await;
    });

    let mut roots = RootCertStore::empty();
    let ca_path = prepared.auto_https.cert_dir.join("local-ca.crt");
    let mut reader = StdBufReader::new(File::open(ca_path).unwrap());
    for certificate in rustls_pemfile::certs(&mut reader) {
        roots.add(certificate.unwrap()).unwrap();
    }
    let client_config =
        ClientConfig::builder_with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])
            .with_root_certificates(roots)
            .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(client_config));
    let domain = TlsServerName::try_from("localhost").unwrap();
    let mut client = connector.connect(domain, client_io).await.unwrap();
    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    if let Err(error) = client.read_to_end(&mut response).await {
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(
        response.ends_with("secure:https:on:localhost\n"),
        "{response}"
    );
    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn public_names_enqueue_acme_but_mixed_names_do_not() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18083;
            auto_https_https_port 18445;
            events {}
            http {
                server {
                    listen 127.0.0.1:18083;
                    server_name public.starling.dev;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let orders = auto_https.orders();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].domains, ["public.starling.dev"]);
    let _ = std::fs::remove_dir_all(prefix);

    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18084;
            auto_https_https_port 18446;
            events {}
            http {
                server {
                    listen 127.0.0.1:18084;
                    server_name localhost public.starling.dev;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    let preparation = prepare_auto_https(&mut prepared, &auto_https).unwrap();
    assert!(auto_https.orders().is_empty());
    assert!(
        preparation
            .notices
            .iter()
            .any(|(_, message)| message.contains("mixes local and public names"))
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn dns01_enqueues_public_wildcard_names() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18088;
            auto_https_https_port 18449;
            auto_https_challenge dns-01;
            auto_https_dns_hook hooks/dns;
            auto_https_dns_propagation_delay 0s;
            events {}
            http {
                server {
                    listen 127.0.0.1:18088;
                    server_name *.starling.dev;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let orders = auto_https.orders();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].domains, ["*.starling.dev"]);
    assert_eq!(orders[0].challenge.as_str(), "dns-01");
    assert_eq!(orders[0].dns_hook, Some(prepared.prefix.join("hooks/dns")));
    assert!(orders[0].dns_propagation_delay.is_zero());
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn generated_https_listener_preserves_default_server() {
    let (prefix, config) = test_config(
        r#"
            auto_https_http_port 18085;
            auto_https_https_port 18447;
            events {}
            http {
                server {
                    listen 127.0.0.1:18085;
                    server_name localhost;
                }
                server {
                    listen 127.0.0.1:18085 default_server;
                    server_name dev.local;
                }
            }
        "#,
        &[],
    );
    let mut prepared = (*config).clone();
    let auto_https = Arc::new(AutoHttpsState::new(prepared.auto_https.clone()));
    prepare_auto_https(&mut prepared, &auto_https).unwrap();
    let https_group = build_listener_groups(&prepared)
        .into_iter()
        .find(|group| group.address.port() == 18447)
        .unwrap();
    assert_eq!(https_group.default_server_id, 1);
    let _ = std::fs::remove_dir_all(prefix);
}

async fn request_for_group(
    config: &Arc<RuntimeConfig>,
    state: SharedState,
    group: ListenerGroup,
    request: &str,
) -> Vec<u8> {
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
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
    let _ = config;
    response
}
