use super::*;

const HTTP2_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

enum PlaintextProtocol<S> {
    Http2(PrefixedStream<S>),
    Http1(PrefixedStream<S>),
}

pub(in crate::server) async fn handle_plaintext_http_connection<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    nodelay: Option<ClientSocketControl>,
    mut shutdown: Option<watch::Receiver<ShutdownMode>>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let timeout = {
        let config = state.current_config();
        connection_server_context(&config, &group)
            .map(|server| server.context.client_header_timeout)
            .unwrap_or_else(|| Duration::from_secs(60))
    };
    let remote = connection.remote.clone();
    let detection = timeout_io(timeout, detect_plaintext_protocol(stream));
    let shutdown_enabled = shutdown.is_some();
    let detection = tokio::select! {
        detection = detection => detection,
        _ = wait_for_optional_shutdown(&mut shutdown), if shutdown_enabled => return,
    };
    let protocol = match detection {
        Ok(protocol) => protocol,
        Err(err) => {
            state.logger.log(
                LogLevel::Info,
                format!("plain HTTP/2 negotiation from {remote} failed: {err}"),
            );
            return;
        }
    };

    match protocol {
        PlaintextProtocol::Http2(stream) => {
            handle_h2(stream, connection, group, state, nodelay, shutdown).await;
        }
        PlaintextProtocol::Http1(stream) => {
            handle_io_with_shutdown(
                stream,
                connection,
                group,
                state,
                reset_target,
                nodelay,
                shutdown,
            )
            .await;
        }
    }
}

async fn detect_plaintext_protocol<S>(mut stream: S) -> io::Result<PlaintextProtocol<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut prefix = Vec::with_capacity(HTTP2_PREFACE.len());
    let mut buffer = [0_u8; HTTP2_PREFACE.len()];
    while prefix.len() < HTTP2_PREFACE.len() {
        let offset = prefix.len();
        let read = stream.read(&mut buffer[offset..]).await?;
        if read == 0 {
            return Ok(PlaintextProtocol::Http1(PrefixedStream::new(
                stream, prefix,
            )));
        }
        prefix.extend_from_slice(&buffer[offset..offset + read]);
        if !HTTP2_PREFACE.starts_with(&prefix) {
            return Ok(PlaintextProtocol::Http1(PrefixedStream::new(
                stream, prefix,
            )));
        }
    }
    Ok(PlaintextProtocol::Http2(PrefixedStream::new(
        stream, prefix,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_a_fragmented_http2_preface() {
        let (mut client, server) = tokio::io::duplex(64);
        let detection = tokio::spawn(detect_plaintext_protocol(server));

        client.write_all(&HTTP2_PREFACE[..7]).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!detection.is_finished());

        client.write_all(&HTTP2_PREFACE[7..]).await.unwrap();
        assert!(matches!(
            detection.await.unwrap().unwrap(),
            PlaintextProtocol::Http2(_)
        ));
    }

    #[tokio::test]
    async fn preserves_bytes_for_http1_fallback() {
        let (mut client, server) = tokio::io::duplex(64);
        let request = b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n";
        client.write_all(request).await.unwrap();
        client.shutdown().await.unwrap();

        let PlaintextProtocol::Http1(mut stream) = detect_plaintext_protocol(server).await.unwrap()
        else {
            panic!("expected HTTP/1 fallback");
        };
        let mut received = Vec::new();
        stream.read_to_end(&mut received).await.unwrap();
        assert_eq!(received, request);
    }
}
