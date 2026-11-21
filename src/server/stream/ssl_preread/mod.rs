use super::*;

mod client_hello;
mod record;
#[cfg(test)]
mod tests;
mod wire;

use record::{ParseSslPreread, parse_ssl_preread};

#[derive(Debug, Clone, Default)]
pub(in crate::server) struct StreamSslPreread {
    pub(in crate::server) protocol: String,
    pub(in crate::server) server_name: String,
    pub(in crate::server) alpn_protocols: String,
}

pub(in crate::server) async fn read_stream_ssl_preread<R>(
    reader: &mut R,
    max_size: usize,
) -> io::Result<(Option<StreamSslPreread>, Vec<u8>)>
where
    R: AsyncRead + Unpin,
{
    let mut buffer = Vec::with_capacity(max_size.min(4096));
    loop {
        match parse_ssl_preread(&buffer) {
            ParseSslPreread::Complete(context) => return Ok((context, buffer)),
            ParseSslPreread::NeedMore => {}
        }
        if buffer.len() >= max_size {
            return Ok((None, buffer));
        }
        let mut chunk = [0_u8; 4096];
        let limit = (max_size - buffer.len()).min(chunk.len());
        let read = reader.read(&mut chunk[..limit]).await?;
        if read == 0 {
            return Ok((None, buffer));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
}

pub(in crate::server) async fn read_stream_ssl_preread_with_timeout<R>(
    reader: &mut R,
    max_size: usize,
    timeout: Duration,
) -> io::Result<(Option<StreamSslPreread>, Vec<u8>)>
where
    R: AsyncRead + Unpin,
{
    if timeout.is_zero() {
        return read_stream_ssl_preread(reader, max_size).await;
    }
    tokio::time::timeout(timeout, read_stream_ssl_preread(reader, max_size))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "stream preread timed out"))?
}
