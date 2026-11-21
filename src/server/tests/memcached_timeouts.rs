use super::support::*;
use super::*;
use std::pin::Pin;
use std::task::{Context as TaskContext, Poll};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, ReadBuf};

#[tokio::test]
async fn send_timeout_is_independent_from_a_shorter_read_timeout() {
    let (prefix, config) = memcached_timeout_config(18170, "1s", "10ms");
    let context = &config.http.as_ref().unwrap().servers[0].context;
    let (client, peer) = tokio::io::duplex(4096);
    let io = DelayedFirstWrite::new(client, Duration::from_millis(40));
    let mut connection = UpstreamConnection::new(Box::new(io), memcached_timeout_key());
    let backend = tokio::spawn(async move {
        let mut peer = tokio::io::BufReader::new(peer);
        let mut request = Vec::new();
        peer.read_until(b'\n', &mut request).await.unwrap();
        assert_eq!(request, b"get item\r\n");
        peer.get_mut().write_all(b"END\r\n").await.unwrap();
    });

    let response = memcached_exchange_connection(&mut connection, b"item", context)
        .await
        .unwrap();

    assert_eq!(response, MemcachedResponse::NotFound);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn read_timeout_is_applied_after_the_request_is_sent() {
    let (prefix, config) = memcached_timeout_config(18171, "1s", "20ms");
    let context = &config.http.as_ref().unwrap().servers[0].context;
    let (client, _peer) = tokio::io::duplex(4096);
    let mut connection = UpstreamConnection::new(Box::new(client), memcached_timeout_key());
    let started = Instant::now();

    let result = memcached_exchange_connection(&mut connection, b"item", context).await;

    assert!(matches!(result, Err(ProxyFailure::Timeout)));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "memcached_read_timeout should expire without waiting for the send timeout"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

fn memcached_timeout_config(
    port: u16,
    send_timeout: &str,
    read_timeout: &str,
) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        memcached_send_timeout {send_timeout};
                        memcached_read_timeout {read_timeout};
                    }}
                }}
            "#
        ),
        &[],
    )
}

fn memcached_timeout_key() -> UpstreamPoolKey {
    UpstreamPoolKey {
        generation: 0,
        scheme: "memcached".into(),
        host: "127.0.0.1".into(),
        port: 11211,
        tls_name: None,
        tls_verify: false,
        tls_trusted_certificate: None,
        tls_verify_depth: None,
        tls_crl: None,
        tls_client_certificate: None,
        tls_client_certificate_key: None,
        tls_protocols: None,
        tls_ciphers: None,
        proxy_bind: None,
        socket_keepalive: false,
        socket_rcvbuf: 0,
        socket_sndbuf: 0,
        http_version: "memcached".into(),
    }
}

struct DelayedFirstWrite {
    inner: tokio::io::DuplexStream,
    delay: Pin<Box<tokio::time::Sleep>>,
    delayed: bool,
}

impl DelayedFirstWrite {
    fn new(inner: tokio::io::DuplexStream, delay: Duration) -> Self {
        Self {
            inner,
            delay: Box::pin(tokio::time::sleep(delay)),
            delayed: false,
        }
    }
}

impl AsyncRead for DelayedFirstWrite {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buffer)
    }
}

impl AsyncWrite for DelayedFirstWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        if !self.delayed {
            std::task::ready!(self.delay.as_mut().poll(cx));
            self.delayed = true;
        }
        Pin::new(&mut self.inner).poll_write(cx, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl UpstreamIo for DelayedFirstWrite {
    fn idle_is_open(&mut self) -> bool {
        true
    }
}
