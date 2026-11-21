use super::*;
use tokio::io::AsyncWriteExt;

pub(super) async fn store_response(
    zone: &ProxyCacheZone,
    key: &str,
    response: &Response,
    validity: &CacheValidity,
) -> io::Result<()> {
    let path = zone.entry_path(key);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;

    let temporary = temporary_path(parent, "cache");
    let created = unix_time_millis();
    let expires = created.saturating_add(duration_millis(validity.valid));
    let (last_modified, etag) = validators(response);
    let result = async {
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .await?;
        entry::write_header(
            &mut output,
            entry::CacheEntryHeader {
                status: response.status,
                headers: &response.headers,
                created,
                expires,
                stale: validity.stale,
                body_length: response.body_len(),
                validators: entry::CacheValidators {
                    last_modified,
                    etag: etag.as_deref(),
                },
            },
        )
        .await?;
        copy_response_body(&response.body, &mut output).await?;
        output.flush().await?;
        drop(output);
        replace_file(&temporary, &path).await?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

pub(super) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(super) fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

pub(super) fn validators(response: &Response) -> (Option<u64>, Option<String>) {
    if response.status != 200 {
        return (None, None);
    }
    let headers = response.upstream_headers();
    let last_modified = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("last-modified"))
        .and_then(|header| httpdate::parse_http_date(header.value.trim()).ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let etag = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("etag"))
        .map(|header| header.value.trim().to_string())
        .filter(|value| !value.is_empty() && value.len() <= entry::MAX_ETAG_BYTES);
    (last_modified, etag)
}
