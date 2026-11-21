use super::store::{duration_millis, unix_time_millis, validators};
use super::*;
use tokio::io::AsyncWriteExt;

pub(super) fn attach_tee(
    zone: &ProxyCacheZone,
    cache_key: &str,
    mut response: Response,
    validity: &CacheValidity,
    guard: Option<CacheLockGuard>,
) -> Response {
    let stream = match std::mem::replace(&mut response.body, ResponseBody::Empty) {
        ResponseBody::Stream(stream) => stream,
        body => {
            response.body = body;
            return response;
        }
    };
    let path = zone.entry_path(cache_key);
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let temporary = temporary_path(&parent, "cache");
    let created = unix_time_millis();
    let expires = created.saturating_add(duration_millis(validity.valid));
    let stale = validity.stale;
    let status = response.status;
    let headers = response.headers.clone();
    let (last_modified, etag) = validators(&response);
    let expected_length = stream.length();
    let prepare = async move {
        tokio::fs::create_dir_all(&parent).await?;
        let cleanup = TemporaryFileGuard::new(temporary.clone());
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .await?;
        async {
            entry::write_header(
                &mut output,
                entry::CacheEntryHeader {
                    status,
                    headers: &headers,
                    created,
                    expires,
                    stale,
                    body_length: expected_length.unwrap_or(0),
                    validators: entry::CacheValidators {
                        last_modified,
                        etag: etag.as_deref(),
                    },
                },
            )
            .await?;
            Ok::<_, io::Error>(CacheTeeSink {
                output: Some(output),
                cleanup,
                temporary: temporary.clone(),
                path,
                expected_length,
                written: 0,
                _guard: guard,
            })
        }
        .await
    };
    response.body = ResponseBody::Stream(tee_response_stream(stream, prepare));
    response
}

struct CacheTeeSink {
    output: Option<tokio::fs::File>,
    cleanup: TemporaryFileGuard,
    temporary: PathBuf,
    path: PathBuf,
    expected_length: Option<u64>,
    written: u64,
    _guard: Option<CacheLockGuard>,
}

#[allow(clippy::manual_async_fn)]
impl TeeSink for CacheTeeSink {
    fn write<'a>(
        &'a mut self,
        data: &'a [u8],
    ) -> impl std::future::Future<Output = io::Result<()>> + Send + 'a {
        async move {
            let length = data.len() as u64;
            let written = self.written.saturating_add(length);
            if self
                .expected_length
                .is_some_and(|expected| written > expected)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "response body exceeds Content-Length",
                ));
            }
            self.output
                .as_mut()
                .expect("cache tee output")
                .write_all(data)
                .await?;
            self.written = written;
            Ok(())
        }
    }

    fn abort(&mut self) -> impl std::future::Future<Output = ()> + Send {
        async move {
            self.output = None;
            let _ = tokio::fs::remove_file(&self.temporary).await;
        }
    }

    fn finish(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send {
        async move {
            if self
                .expected_length
                .is_some_and(|expected| expected != self.written)
            {
                self.abort().await;
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "response body ended before Content-Length",
                ));
            }
            let Some(mut output) = self.output.take() else {
                return Ok(());
            };
            let result = async {
                output.flush().await?;
                entry::write_body_length(&mut output, self.written).await?;
                drop(output);
                replace_file(&self.temporary, &self.path).await
            }
            .await;
            if result.is_ok() {
                self.cleanup.disarm();
            }
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProxyCacheLockConfig;
    use crate::http::{ResponseStream, ResponseStreamItem};
    use tokio::sync::mpsc;

    fn zone(path: PathBuf) -> ProxyCacheZone {
        ProxyCacheZone::new(crate::config::ProxyCachePathConfig {
            path,
            levels: [0; 3],
            keys_zone: "tee_test".into(),
            keys_zone_size: 1024,
            inactive: Duration::from_secs(60),
            max_size: None,
            use_temp_path: false,
            loader_files: 1,
            loader_sleep: Duration::ZERO,
            loader_threshold: Duration::ZERO,
            manager_files: 1,
            manager_sleep: Duration::ZERO,
            manager_threshold: Duration::ZERO,
            min_free: None,
        })
    }

    fn streamed_response(
        length: Option<u64>,
    ) -> (mpsc::Sender<io::Result<ResponseStreamItem>>, Response) {
        let (sender, receiver) = mpsc::channel(8);
        let mut response = Response::new(200);
        response.body = ResponseBody::Stream(ResponseStream::new(receiver, length, false));
        (sender, response)
    }

    async fn wait_for_lock_release(zone: &ProxyCacheZone, key: &str) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while zone.is_updating(key) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cache lock should be released after tee failure");
    }

    #[tokio::test]
    async fn prepare_failure_releases_lock_without_truncating_downstream() {
        let root =
            std::env::temp_dir().join(format!("starling-cache-tee-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let blocked = root.join("blocked");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let zone = zone(blocked);
        let key = "GET /prepare-failure";
        let lock = ProxyCacheLockConfig {
            enabled: true,
            timeout: Duration::from_secs(1),
            age: Duration::from_secs(5),
        };
        let CacheLockOutcome::Acquired(guard) = zone.acquire_lock(key, &lock).await else {
            panic!("cache lock should be acquired");
        };
        let (sender, response) = streamed_response(Some(5));
        let response = zone
            .store_or_tee(
                key,
                response,
                &CacheValidity::new(Duration::from_secs(60), CacheStaleWindows::default()),
                Some(guard),
            )
            .await;
        assert!(zone.is_updating(key));

        sender
            .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
            .await
            .unwrap();
        drop(sender);
        let ResponseBody::Stream(mut body) = response.body else {
            panic!("expected streamed cache response");
        };
        let item = body.next().await.unwrap().expect("downstream data");
        let ResponseStreamItem::Data(data) = item else {
            panic!("expected downstream data");
        };
        assert_eq!(&data[..], b"first");
        assert!(body.next().await.unwrap().is_none());
        wait_for_lock_release(&zone, key).await;
        let error = match zone.lookup(key).await {
            Err(error) => error,
            Ok(_) => panic!("blocked cache path must not produce a cache lookup"),
        };
        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
        drop(zone);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_only_cache_directory_releases_lock_and_keeps_downstream_complete() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = std::env::temp_dir().join(format!(
            "starling-cache-tee-permissions-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let cache = root.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let zone = zone(cache.clone());
        let key = "GET /read-only-cache";
        let lock = ProxyCacheLockConfig {
            enabled: true,
            timeout: Duration::from_secs(1),
            age: Duration::from_secs(5),
        };
        let CacheLockOutcome::Acquired(guard) = zone.acquire_lock(key, &lock).await else {
            panic!("cache lock should be acquired");
        };
        let (sender, response) = streamed_response(Some(5));
        let response = zone
            .store_or_tee(
                key,
                response,
                &CacheValidity::new(Duration::from_secs(60), CacheStaleWindows::default()),
                Some(guard),
            )
            .await;
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o500)).unwrap();
        sender
            .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
            .await
            .unwrap();
        drop(sender);

        let ResponseBody::Stream(mut body) = response.body else {
            panic!("expected streamed cache response");
        };
        let mut output = Vec::new();
        while let Some(item) = body.next().await.unwrap() {
            if let ResponseStreamItem::Data(data) = item {
                output.extend_from_slice(&data);
            }
        }
        assert_eq!(output, b"first");
        wait_for_lock_release(&zone, key).await;
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(zone.lookup(key).await.unwrap(), CacheLookup::Miss));
        drop(zone);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn write_failure_releases_lock_and_keeps_downstream_complete() {
        let root =
            std::env::temp_dir().join(format!("starling-cache-tee-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let zone = zone(root.join("cache"));
        let key = "GET /write-failure";
        let lock = ProxyCacheLockConfig {
            enabled: true,
            timeout: Duration::from_secs(1),
            age: Duration::from_secs(5),
        };
        let CacheLockOutcome::Acquired(guard) = zone.acquire_lock(key, &lock).await else {
            panic!("cache lock should be acquired");
        };
        let (sender, response) = streamed_response(Some(3));
        let response = zone
            .store_or_tee(
                key,
                response,
                &CacheValidity::new(Duration::from_secs(60), CacheStaleWindows::default()),
                Some(guard),
            )
            .await;
        assert!(zone.is_updating(key));

        for chunk in [b"four".as_slice(), b"five"] {
            sender
                .send(Ok(ResponseStreamItem::Data(Bytes::copy_from_slice(chunk))))
                .await
                .unwrap();
        }
        drop(sender);
        let ResponseBody::Stream(mut body) = response.body else {
            panic!("expected streamed cache response");
        };
        let mut output = Vec::new();
        while let Some(item) = body.next().await.unwrap() {
            if let ResponseStreamItem::Data(data) = item {
                output.extend_from_slice(&data);
            }
        }
        assert_eq!(output, b"fourfive");
        wait_for_lock_release(&zone, key).await;
        assert!(matches!(zone.lookup(key).await.unwrap(), CacheLookup::Miss));
        drop(zone);
        let _ = std::fs::remove_dir_all(root);
    }
}
