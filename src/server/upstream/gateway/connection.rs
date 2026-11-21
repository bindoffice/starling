use super::*;

pub(in crate::server) async fn open_gateway_io(
    host: &str,
    port: u16,
    connect_timeout: Duration,
    socket_options: UpstreamSocketOptions,
    local_bind: Option<&str>,
    bind_directive: &str,
) -> Result<BoxedUpstreamIo, ProxyFailure> {
    if let Some(path) = host.strip_prefix("unix:") {
        #[cfg(unix)]
        {
            let connect = tokio::net::UnixStream::connect(path);
            let stream = match tokio::time::timeout(connect_timeout, connect).await {
                Ok(Ok(stream)) => stream,
                Ok(Err(_)) => return Err(ProxyFailure::Failure),
                Err(_) => return Err(ProxyFailure::Timeout),
            };
            if apply_upstream_socket_options(&stream, socket_options).is_err() {
                return Err(ProxyFailure::Failure);
            }
            return Ok(Box::new(stream));
        }
        #[cfg(not(unix))]
        {
            let _ = path;
            return Err(ProxyFailure::Failure);
        }
    }
    let connect = connect_upstream_tcp(host, port, local_bind, socket_options, bind_directive);
    match tokio::time::timeout(connect_timeout, connect).await {
        Ok(Ok(stream)) => Ok(Box::new(stream)),
        Ok(Err(_)) => Err(ProxyFailure::Failure),
        Err(_) => Err(ProxyFailure::Timeout),
    }
}
