use super::*;

mod body_spool;
mod body_storage;
mod body_stream;
mod head;

pub use body_spool::*;
pub use body_storage::*;
pub use body_stream::*;
pub use head::*;

pub async fn read_request_body<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    head: &RequestHead,
    limit: u64,
) -> io::Result<Vec<u8>> {
    if head.chunked {
        return read_chunked_body(reader, limit, None).await;
    }
    let Some(length) = head.content_length else {
        return Ok(Vec::new());
    };
    if length > limit || length > MAX_BODY_BYTES as u64 {
        return Err(request_body_too_large_error());
    }
    let mut body = vec![0; length as usize];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

pub(in crate::http) async fn read_chunked_body<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    limit: u64,
    rate: Option<&mut ReadRate>,
) -> io::Result<Vec<u8>> {
    Ok(read_chunked_body_with_trailers(reader, limit, rate)
        .await?
        .0)
}

pub(in crate::http) async fn read_chunked_body_with_trailers<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    limit: u64,
    mut rate: Option<&mut ReadRate>,
) -> io::Result<(Vec<u8>, Vec<Header>)> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Err(invalid_data("unexpected end of chunked body"));
        }
        let size_text = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| invalid_data("invalid chunk size"))?;
        if size == 0 {
            let mut trailers = Vec::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).await? == 0 {
                    return Err(invalid_data("unexpected end of chunked trailers"));
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                let line = line.trim_end_matches(['\r', '\n']);
                let Some((name, value)) = line.split_once(':') else {
                    return Err(invalid_data("invalid chunked trailer"));
                };
                if name.is_empty() || !name.bytes().all(is_token_byte) {
                    return Err(invalid_data("invalid chunked trailer"));
                }
                trailers.push(Header {
                    name: name.into(),
                    value: value.trim().to_string(),
                });
            }
            return Ok((body, trailers));
        }
        if body.len().saturating_add(size) > limit as usize
            || body.len().saturating_add(size) > MAX_BODY_BYTES
        {
            return Err(request_body_too_large_error());
        }
        let start = body.len();
        body.resize(start + size, 0);
        reader.read_exact(&mut body[start..]).await?;
        if let Some(rate) = rate.as_deref_mut() {
            rate.account(size as u64).await;
        }
        let mut crlf = [0_u8; 2];
        reader.read_exact(&mut crlf).await?;
        if crlf != *b"\r\n" {
            return Err(invalid_data("invalid chunk terminator"));
        }
    }
}
