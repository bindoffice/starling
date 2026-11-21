use super::*;

pub(in crate::server) async fn copy_stream_bidirectional<A, B>(
    inbound: &mut A,
    outbound: &mut B,
    half_close: bool,
) -> io::Result<()>
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    if half_close {
        tokio::io::copy_bidirectional(inbound, outbound)
            .await
            .map(|_| ())
    } else {
        let (mut inbound_read, mut inbound_write) = tokio::io::split(inbound);
        let (mut outbound_read, mut outbound_write) = tokio::io::split(outbound);
        let upload = tokio::io::copy(&mut inbound_read, &mut outbound_write);
        let download = tokio::io::copy(&mut outbound_read, &mut inbound_write);
        tokio::pin!(upload);
        tokio::pin!(download);
        tokio::select! {
            result = &mut upload => result.map(|_| ()),
            result = &mut download => result.map(|_| ()),
        }
    }
}
