use std::io;

use tokio::io::{AsyncBufReadExt, AsyncReadExt};

use super::buffer::BodyBuffer;
use crate::http::{Header, ReadRate, invalid_data, is_token_byte};

const READ_CHUNK_SIZE: usize = 16 * 1024;

pub(super) async fn read_content_length_body<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    length: u64,
    body: &mut BodyBuffer,
    mut rate: Option<&mut ReadRate>,
) -> io::Result<()> {
    let mut remaining = length;
    let mut buffer = [0_u8; READ_CHUNK_SIZE];
    while remaining > 0 {
        let chunk_len = remaining.min(buffer.len() as u64) as usize;
        reader.read_exact(&mut buffer[..chunk_len]).await?;
        body.append(&buffer[..chunk_len]).await?;
        if let Some(rate) = rate.as_deref_mut() {
            rate.account(chunk_len as u64).await;
        }
        remaining -= chunk_len as u64;
    }
    Ok(())
}

pub(super) async fn read_to_end_body<R: AsyncReadExt + Unpin>(
    reader: &mut R,
    body: &mut BodyBuffer,
    mut rate: Option<&mut ReadRate>,
) -> io::Result<()> {
    let mut buffer = [0_u8; READ_CHUNK_SIZE];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }
        body.append(&buffer[..read]).await?;
        if let Some(rate) = rate.as_deref_mut() {
            rate.account(read as u64).await;
        }
    }
}

pub(super) async fn read_chunked_body<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    body: &mut BodyBuffer,
    mut rate: Option<&mut ReadRate>,
) -> io::Result<Vec<Header>> {
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            return Err(invalid_data("unexpected end of chunked body"));
        }
        let size_text = line
            .trim_end_matches(['\r', '\n'])
            .split(';')
            .next()
            .unwrap_or_default();
        let size = u64::from_str_radix(size_text.trim(), 16)
            .map_err(|_| invalid_data("invalid chunk size"))?;
        if size == 0 {
            return read_trailers(reader).await;
        }

        let mut remaining = size;
        let mut buffer = [0_u8; READ_CHUNK_SIZE];
        while remaining > 0 {
            let chunk_len = remaining.min(buffer.len() as u64) as usize;
            reader.read_exact(&mut buffer[..chunk_len]).await?;
            body.append(&buffer[..chunk_len]).await?;
            if let Some(rate) = rate.as_deref_mut() {
                rate.account(chunk_len as u64).await;
            }
            remaining -= chunk_len as u64;
        }

        let mut crlf = [0_u8; 2];
        reader.read_exact(&mut crlf).await?;
        if crlf != *b"\r\n" {
            return Err(invalid_data("invalid chunk terminator"));
        }
    }
}

pub(crate) async fn read_trailers<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> io::Result<Vec<Header>> {
    let mut trailers = Vec::new();
    loop {
        let mut line = String::new();
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
            name: name.to_string(),
            value: value.trim().to_string(),
        });
    }
    Ok(trailers)
}
