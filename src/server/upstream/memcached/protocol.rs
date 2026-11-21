use super::*;
use tokio::io::AsyncBufReadExt;

const MEMCACHED_TRAILER: &[u8; 7] = b"\r\nEND\r\n";

#[derive(Debug, PartialEq, Eq)]
pub(in crate::server) enum MemcachedResponse {
    Value { body: Vec<u8>, gzip: bool },
    NotFound,
}

pub(in crate::server) enum MemcachedHead {
    Value { length: u64, gzip: bool },
    NotFound,
}

pub(in crate::server) fn build_memcached_request(key: &[u8]) -> Vec<u8> {
    let mut request = Vec::with_capacity(key.len() + 6);
    request.extend_from_slice(b"get ");
    request.extend_from_slice(key);
    request.extend_from_slice(b"\r\n");
    request
}

pub(in crate::server) fn escape_memcached_key(key: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";

    let mut escaped = Vec::with_capacity(key.len());
    for &byte in key {
        if byte <= b' ' || byte == b'%' {
            escaped.push(b'%');
            escaped.push(HEX[(byte >> 4) as usize]);
            escaped.push(HEX[(byte & 0x0f) as usize]);
        } else {
            escaped.push(byte);
        }
    }
    escaped
}

#[cfg(test)]
pub(in crate::server) async fn read_memcached_response<R>(
    reader: &mut R,
    expected_key: &[u8],
    gzip_flag: u32,
    max_line_bytes: usize,
) -> io::Result<MemcachedResponse>
where
    R: AsyncBufRead + Unpin,
{
    let head = read_memcached_head(reader, expected_key, gzip_flag, max_line_bytes).await?;
    match head {
        MemcachedHead::NotFound => Ok(MemcachedResponse::NotFound),
        MemcachedHead::Value { length, gzip } => {
            let body = read_memcached_body(reader, length).await?;
            read_memcached_trailer(reader).await?;
            Ok(MemcachedResponse::Value { body, gzip })
        }
    }
}

pub(in crate::server) async fn read_memcached_head<R>(
    reader: &mut R,
    expected_key: &[u8],
    gzip_flag: u32,
    max_line_bytes: usize,
) -> io::Result<MemcachedHead>
where
    R: AsyncBufRead + Unpin,
{
    let line = read_line_bounded(reader, max_line_bytes).await?;
    let line = line.strip_suffix(b"\r\n").ok_or_else(invalid_response)?;
    if line == b"END" {
        return Ok(MemcachedHead::NotFound);
    }

    let Some(value) = line.strip_prefix(b"VALUE ") else {
        return Err(invalid_response());
    };
    let Some((key, fields)) = split_once_space(value) else {
        return Err(invalid_response());
    };
    if key != expected_key {
        return Err(invalid_response());
    }
    let Some((flags, length)) = split_once_space(fields) else {
        return Err(invalid_response());
    };

    let gzip = if gzip_flag == 0 {
        false
    } else {
        parse_decimal_u32(flags)? & gzip_flag != 0
    };
    let length = parse_decimal_u64(length)?;
    Ok(MemcachedHead::Value { length, gzip })
}

pub(in crate::server) async fn read_memcached_body<R>(
    reader: &mut R,
    length: u64,
) -> io::Result<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let length = usize::try_from(length).map_err(|_| invalid_response())?;
    let mut body = Vec::new();
    body.try_reserve_exact(length)
        .map_err(|_| invalid_response())?;
    body.resize(length, 0);
    reader.read_exact(&mut body).await?;
    Ok(body)
}

pub(in crate::server) async fn read_memcached_trailer<R>(reader: &mut R) -> io::Result<()>
where
    R: AsyncRead + Unpin,
{
    let mut trailer = [0_u8; MEMCACHED_TRAILER.len()];
    reader.read_exact(&mut trailer).await?;
    if trailer != *MEMCACHED_TRAILER {
        return Err(invalid_response());
    }
    Ok(())
}

async fn read_line_bounded<R>(reader: &mut R, max_line_bytes: usize) -> io::Result<Vec<u8>>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "memcached response ended before a complete line",
            ));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(take) > max_line_bytes {
            reader.consume(take);
            return Err(invalid_response());
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            return Ok(line);
        }
    }
}

fn split_once_space(value: &[u8]) -> Option<(&[u8], &[u8])> {
    let index = value.iter().position(|byte| *byte == b' ')?;
    Some((&value[..index], &value[index + 1..]))
}

fn parse_decimal_u32(value: &[u8]) -> io::Result<u32> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(invalid_response());
    }
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(invalid_response)
}

fn parse_decimal_u64(value: &[u8]) -> io::Result<u64> {
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return Err(invalid_response());
    }
    std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or_else(invalid_response)
}

fn invalid_response() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "invalid memcached response")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_memcached_keys_using_nginx_rules() {
        assert_eq!(
            escape_memcached_key(b"alpha beta%gamma\x01"),
            b"alpha%20beta%25gamma%01"
        );
        assert_eq!(escape_memcached_key(b"safe-._~"), b"safe-._~".to_vec());
    }

    #[tokio::test]
    async fn parses_value_and_not_found_responses() {
        let mut value = tokio::io::BufReader::new(&b"VALUE cache-key 8 5\r\nhello\r\nEND\r\n"[..]);
        assert_eq!(
            read_memcached_response(&mut value, b"cache-key", 8, 4096)
                .await
                .unwrap(),
            MemcachedResponse::Value {
                body: b"hello".to_vec(),
                gzip: true,
            }
        );

        let mut missing = tokio::io::BufReader::new(&b"END\r\n"[..]);
        assert_eq!(
            read_memcached_response(&mut missing, b"cache-key", 0, 4096)
                .await
                .unwrap(),
            MemcachedResponse::NotFound
        );
    }

    #[tokio::test]
    async fn rejects_wrong_keys_and_trailers() {
        let mut wrong_key = tokio::io::BufReader::new(&b"VALUE other 0 2\r\nok\r\nEND\r\n"[..]);
        let error = read_memcached_response(&mut wrong_key, b"cache-key", 0, 4096)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut wrong_trailer =
            tokio::io::BufReader::new(&b"VALUE cache-key 0 2\r\nok\r\nEND\rX"[..]);
        let error = read_memcached_response(&mut wrong_trailer, b"cache-key", 0, 4096)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
