use super::*;

pub(in crate::server) async fn handle_io<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    handle_io_with_reset(stream, connection, group, state, None, None).await;
}

pub(in crate::server) async fn handle_io_with_reset<S>(
    stream: S,
    connection: ConnectionAddresses,
    group: ListenerGroup,
    state: SharedState,
    reset_target: Option<TimeoutReset>,
    control: Option<ClientSocketControl>,
) where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    connection::handle_io_with_shutdown(
        stream,
        connection,
        group,
        state,
        reset_target,
        control,
        None,
    )
    .await;
}
