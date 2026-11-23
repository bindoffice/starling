use super::*;
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

const MAGIC: &[u8; 8] = b"STRLC005";
const MAX_CACHE_HEADERS: u32 = 1024;
const MAX_CACHE_HEADER_BYTES: usize = 64 * 1024;
const MAX_CHARSET_BYTES: usize = 255;
pub(super) const MAX_ETAG_BYTES: usize = 128;
const FLAG_CHARSET: u16 = 1;
const CREATED_OFFSET: u64 = 12;
const EXPIRES_OFFSET: u64 = 20;
const STALE_UPDATING_OFFSET: u64 = 28;
const STALE_ERROR_OFFSET: u64 = 36;
pub(super) const BODY_LENGTH_OFFSET: u64 = 44;

// Cache timestamps use Unix milliseconds so sub-second proxy_cache_valid values are preserved.
pub(super) struct CacheEntry {
    pub(super) status: u16,
    pub(super) headers: Vec<Header>,
    pub(super) created: u64,
    pub(super) expires: u64,
    pub(super) stale: CacheStaleWindows,
    pub(super) last_modified: Option<u64>,
    pub(super) etag: Option<String>,
    pub(super) charset: Option<String>,
    pub(super) body: ResponseBody,
}

pub(super) struct CacheValidators<'a> {
    pub(super) last_modified: Option<u64>,
    pub(super) etag: Option<&'a str>,
}

pub(super) struct CacheEntryHeader<'a> {
    pub(super) status: u16,
    pub(super) headers: &'a [Header],
    pub(super) created: u64,
    pub(super) expires: u64,
    pub(super) stale: CacheStaleWindows,
    pub(super) body_length: u64,
    pub(super) validators: CacheValidators<'a>,
    pub(super) charset: Option<&'a str>,
}

pub(super) async fn is_cache_entry(path: &Path) -> io::Result<bool> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut magic = [0_u8; 8];
    if file.read_exact(&mut magic).await.is_err() || &magic != MAGIC {
        return Ok(false);
    }
    if file.read_u16().await.is_err() {
        return Ok(false);
    }
    let flags = match file.read_u16().await {
        Ok(flags) => flags,
        Err(_) => return Ok(false),
    };
    if file.read_u64().await.is_err() {
        return Ok(false);
    }
    let _expires = match file.read_u64().await {
        Ok(expires) => expires,
        Err(_) => return Ok(false),
    };
    if file.read_u64().await.is_err() || file.read_u64().await.is_err() {
        return Ok(false);
    }
    let body_length = match file.read_u64().await {
        Ok(length) => length,
        Err(_) => return Ok(false),
    };
    if file.read_i64().await.is_err() {
        return Ok(false);
    }
    let etag_length = match file.read_u16().await {
        Ok(length) if usize::from(length) <= MAX_ETAG_BYTES => usize::from(length),
        _ => return Ok(false),
    };
    let header_count = match file.read_u32().await {
        Ok(count) if count <= MAX_CACHE_HEADERS => count,
        _ => return Ok(false),
    };
    if file
        .seek(SeekFrom::Current(
            i64::try_from(etag_length).unwrap_or(i64::MAX),
        ))
        .await
        .is_err()
    {
        return Ok(false);
    }
    if flags & FLAG_CHARSET != 0 {
        let charset_length = match file.read_u16().await {
            Ok(length) if usize::from(length) <= MAX_CHARSET_BYTES => usize::from(length),
            _ => return Ok(false),
        };
        if file
            .seek(SeekFrom::Current(
                i64::try_from(charset_length).unwrap_or(i64::MAX),
            ))
            .await
            .is_err()
        {
            return Ok(false);
        }
    }
    for _ in 0..header_count {
        let name_length = match file.read_u16().await {
            Ok(length) => u64::from(length),
            Err(_) => return Ok(false),
        };
        let value_length = match file.read_u32().await {
            Ok(length) => u64::from(length),
            Err(_) => return Ok(false),
        };
        let skip = name_length.saturating_add(value_length);
        if file
            .seek(SeekFrom::Current(i64::try_from(skip).unwrap_or(i64::MAX)))
            .await
            .is_err()
        {
            return Ok(false);
        }
    }
    let body_offset = file.stream_position().await?;
    let file_length = file.metadata().await?.len();
    if body_offset
        .checked_add(body_length)
        .is_none_or(|end| end > file_length)
    {
        return Ok(false);
    }
    Ok(true)
}

pub(super) async fn read_entry(path: &Path) -> io::Result<Option<CacheEntry>> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut magic = [0_u8; 8];
    if file.read_exact(&mut magic).await.is_err() || &magic != MAGIC {
        return Ok(None);
    }
    let status = match file.read_u16().await {
        Ok(status) => status,
        Err(_) => return Ok(None),
    };
    let flags = match file.read_u16().await {
        Ok(flags) => flags,
        Err(_) => return Ok(None),
    };
    let created = match file.read_u64().await {
        Ok(created) => created,
        Err(_) => return Ok(None),
    };
    let expires = match file.read_u64().await {
        Ok(expires) => expires,
        Err(_) => return Ok(None),
    };
    let stale_updating = match file.read_u64().await {
        Ok(millis) => millis,
        Err(_) => return Ok(None),
    };
    let stale_error = match file.read_u64().await {
        Ok(millis) => millis,
        Err(_) => return Ok(None),
    };
    let body_length = match file.read_u64().await {
        Ok(length) => length,
        Err(_) => return Ok(None),
    };
    let last_modified = match file.read_i64().await {
        Ok(timestamp) if timestamp >= 0 => Some(timestamp as u64),
        Ok(_) => None,
        Err(_) => return Ok(None),
    };
    let etag_length = match file.read_u16().await {
        Ok(length) if usize::from(length) <= MAX_ETAG_BYTES => usize::from(length),
        _ => return Ok(None),
    };
    let header_count = match file.read_u32().await {
        Ok(count) if count <= MAX_CACHE_HEADERS => count,
        _ => return Ok(None),
    };
    let mut etag = vec![0_u8; etag_length];
    if file.read_exact(&mut etag).await.is_err() {
        return Ok(None);
    }
    let etag = if etag.is_empty() {
        None
    } else {
        match String::from_utf8(etag) {
            Ok(etag) => Some(etag),
            Err(_) => return Ok(None),
        }
    };
    let charset = if flags & FLAG_CHARSET != 0 {
        let length = match file.read_u16().await {
            Ok(length) if usize::from(length) <= MAX_CHARSET_BYTES => usize::from(length),
            _ => return Ok(None),
        };
        let mut charset = vec![0_u8; length];
        if file.read_exact(&mut charset).await.is_err() {
            return Ok(None);
        }
        match String::from_utf8(charset) {
            Ok(charset) if !charset.is_empty() => Some(charset),
            _ => return Ok(None),
        }
    } else {
        None
    };

    let mut headers = Vec::with_capacity(header_count as usize);
    let mut header_bytes = 0_usize;
    for _ in 0..header_count {
        let name_length = match file.read_u16().await {
            Ok(length) => length as usize,
            Err(_) => return Ok(None),
        };
        let value_length = match file.read_u32().await {
            Ok(length) => length as usize,
            Err(_) => return Ok(None),
        };
        header_bytes = header_bytes
            .saturating_add(name_length)
            .saturating_add(value_length);
        if header_bytes > MAX_CACHE_HEADER_BYTES {
            return Ok(None);
        }
        let mut name = vec![0_u8; name_length];
        let mut value = vec![0_u8; value_length];
        if file.read_exact(&mut name).await.is_err() || file.read_exact(&mut value).await.is_err() {
            return Ok(None);
        }
        let (Ok(name), Ok(value)) = (String::from_utf8(name), String::from_utf8(value)) else {
            return Ok(None);
        };
        headers.push(Header { name, value });
    }

    let body_offset = file.stream_position().await?;
    let file_length = file.metadata().await?.len();
    if body_offset
        .checked_add(body_length)
        .is_none_or(|end| end > file_length)
    {
        return Ok(None);
    }
    file.seek(SeekFrom::Start(body_offset)).await?;
    Ok(Some(CacheEntry {
        status,
        headers,
        created,
        expires,
        stale: CacheStaleWindows::from_millis(stale_updating, stale_error),
        last_modified,
        etag,
        charset,
        body: ResponseBody::File {
            file,
            offset: body_offset,
            length: body_length,
        },
    }))
}

pub(super) async fn write_header(
    output: &mut tokio::fs::File,
    header: CacheEntryHeader<'_>,
) -> io::Result<()> {
    let CacheEntryHeader {
        status,
        headers,
        created,
        expires,
        stale,
        body_length,
        validators,
        charset,
    } = header;
    output.write_all(MAGIC).await?;
    output.write_u16(status).await?;
    output.write_u16(u16::from(charset.is_some())).await?;
    output.write_u64(created).await?;
    output.write_u64(expires).await?;
    output
        .write_u64(store::duration_millis(stale.updating))
        .await?;
    output
        .write_u64(store::duration_millis(stale.error))
        .await?;
    output.write_u64(body_length).await?;
    output
        .write_i64(
            validators
                .last_modified
                .map(|time| time as i64)
                .unwrap_or(-1),
        )
        .await?;
    let etag = validators.etag.unwrap_or_default();
    let etag_length = u16::try_from(etag.len())
        .ok()
        .filter(|length| usize::from(*length) <= MAX_ETAG_BYTES)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ETag too long for cache"))?;
    output.write_u16(etag_length).await?;
    let header_count = u32::try_from(headers.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "too many response headers"))?;
    output.write_u32(header_count).await?;
    output.write_all(etag.as_bytes()).await?;
    if let Some(charset) = charset {
        let charset_length = u16::try_from(charset.len())
            .ok()
            .filter(|length| usize::from(*length) <= MAX_CHARSET_BYTES)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "charset too long for cache")
            })?;
        output.write_u16(charset_length).await?;
        output.write_all(charset.as_bytes()).await?;
    }
    let mut header_bytes = 0_usize;
    for header in headers {
        let name_length = u16::try_from(header.name.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "response header name too long")
        })?;
        let value_length = u32::try_from(header.value.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "response header value too long",
            )
        })?;
        header_bytes = header_bytes
            .saturating_add(header.name.len())
            .saturating_add(header.value.len());
        if header_bytes > MAX_CACHE_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "response headers too large for cache",
            ));
        }
        output.write_u16(name_length).await?;
        output.write_u32(value_length).await?;
        output.write_all(header.name.as_bytes()).await?;
        output.write_all(header.value.as_bytes()).await?;
    }
    Ok(())
}

pub(super) async fn update_validity(
    path: &Path,
    expected_created: u64,
    created: u64,
    expires: u64,
    stale: CacheStaleWindows,
) -> io::Result<bool> {
    let mut file = match tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut magic = [0_u8; 8];
    if file.read_exact(&mut magic).await.is_err() || &magic != MAGIC {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(CREATED_OFFSET)).await?;
    let current_created = file.read_u64().await?;
    if current_created != expected_created {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(CREATED_OFFSET)).await?;
    file.write_u64(created).await?;
    file.seek(SeekFrom::Start(EXPIRES_OFFSET)).await?;
    file.write_u64(expires).await?;
    file.seek(SeekFrom::Start(STALE_UPDATING_OFFSET)).await?;
    file.write_u64(store::duration_millis(stale.updating))
        .await?;
    file.seek(SeekFrom::Start(STALE_ERROR_OFFSET)).await?;
    file.write_u64(store::duration_millis(stale.error)).await?;
    file.flush().await?;
    Ok(true)
}

pub(super) async fn write_body_length(output: &mut tokio::fs::File, length: u64) -> io::Result<()> {
    output.seek(SeekFrom::Start(BODY_LENGTH_OFFSET)).await?;
    output.write_u64(length).await?;
    output.flush().await
}
