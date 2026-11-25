use std::io;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use super::body_storage::RequestBodyFile;
use super::body_stream::RequestBodyStream;
use crate::config::{ClientBodyInFileOnly, ClientBodyTempPath};
use crate::http::{TempFileGuard, create_temp_file};

const SPOOL_WRITE_BATCH: usize = 64 * 1024;

pub struct SpooledRequestBody {
    pub body: Vec<u8>,
    pub file: Option<Arc<RequestBodyFile>>,
}

pub async fn spool_request_body(
    stream: Option<&RequestBodyStream>,
    body_declared: bool,
    buffer_size: usize,
    temp_path: &ClientBodyTempPath,
    file_only: ClientBodyInFileOnly,
    limit: u64,
) -> io::Result<SpooledRequestBody> {
    let mut body = Vec::new();
    let mut spool = (file_only.enabled() && body_declared)
        .then(|| SpoolFile::create(temp_path))
        .transpose()?;
    let mut total = 0_u64;

    if let Some(stream) = stream {
        while let Some(chunk) = stream.next_limited(&mut total, limit).await? {
            if spool.is_none() && body.len().saturating_add(chunk.len()) > buffer_size {
                let mut file = SpoolFile::create(temp_path)?;
                if !body.is_empty() {
                    file.write_all(&body)?;
                    body.clear();
                }
                spool = Some(file);
            }
            if let Some(spool) = &mut spool {
                spool.write_all(&chunk)?;
            } else {
                body.extend_from_slice(&chunk);
            }
        }
    }

    let file = match spool {
        Some(spool) => {
            let path = spool.finish()?;
            Some(Arc::new(RequestBodyFile::new(
                TempFileGuard::new(path, file_only.remove_on_drop()),
                total,
            )))
        }
        None => None,
    };

    Ok(SpooledRequestBody { body, file })
}

struct SpoolFile {
    path: Option<PathBuf>,
    file: std::fs::File,
    pending: Vec<u8>,
}

impl SpoolFile {
    fn create(temp_path: &ClientBodyTempPath) -> io::Result<Self> {
        let (path, file) = create_temp_file(temp_path, "request body")?;
        Ok(Self {
            path: Some(path),
            file,
            pending: Vec::with_capacity(SPOOL_WRITE_BATCH),
        })
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        if data.len() >= SPOOL_WRITE_BATCH {
            self.flush_pending()?;
            return self.file.write_all(data);
        }
        if self.pending.len().saturating_add(data.len()) > SPOOL_WRITE_BATCH {
            self.flush_pending()?;
        }
        self.pending.extend_from_slice(data);
        if self.pending.len() >= SPOOL_WRITE_BATCH {
            self.flush_pending()?;
        }
        Ok(())
    }

    fn flush_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let result = self.file.write_all(&self.pending);
        self.pending.clear();
        result
    }

    fn finish(mut self) -> io::Result<PathBuf> {
        self.flush_pending()?;
        self.file.flush()?;
        Ok(self.path.take().expect("request body spool path"))
    }
}

impl Drop for SpoolFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use bytes::Bytes;

    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(1);

    #[tokio::test]
    async fn spills_large_body_and_replays_file_from_the_start() {
        let root = test_root("replay");
        let config = crate::config::ClientBodyConfig::new(root.clone());
        let (sender, stream) = RequestBodyStream::channel();
        let producer = tokio::spawn(async move {
            assert!(sender.send(Bytes::from_static(b"ab")).await);
            assert!(sender.send(Bytes::from_static(b"cdef")).await);
        });
        let result = spool_request_body(
            Some(&stream),
            true,
            4,
            &config.temp_path,
            ClientBodyInFileOnly::Off,
            0,
        )
        .await
        .unwrap();
        producer.await.unwrap();

        assert!(result.body.is_empty());
        let file = result.file.expect("spilled request body");
        assert_eq!(file.len(), 6);
        let first = RequestBodyStream::from_file(file.clone());
        let second = RequestBodyStream::from_file(file.clone());
        assert_eq!(collect(first).await, b"abcdef");
        assert_eq!(collect(second).await, b"abcdef");
        drop(file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn zero_limit_spools_beyond_the_legacy_body_cap() {
        let root = test_root("unlimited");
        let config = crate::config::ClientBodyConfig::new(root.clone());
        let (sender, stream) = RequestBodyStream::channel();
        let producer = tokio::spawn(async move {
            let chunk = Bytes::from(vec![b'x'; 1024 * 1024]);
            for _ in 0..65 {
                assert!(sender.send(chunk.clone()).await);
            }
        });
        let result = spool_request_body(
            Some(&stream),
            true,
            8 * 1024,
            &config.temp_path,
            ClientBodyInFileOnly::Off,
            0,
        )
        .await
        .unwrap();
        producer.await.unwrap();

        assert!(result.body.is_empty());
        let file = result.file.expect("spilled request body");
        assert_eq!(file.len(), 65 * 1024 * 1024);
        assert!(
            file.len() > crate::http::MAX_BODY_BYTES as u64,
            "buffering-on must not retain the old fixed body cap"
        );
        drop(file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn overflow_after_spilling_removes_the_partial_file() {
        let root = test_root("overflow");
        let config = crate::config::ClientBodyConfig::new(root.clone());
        let (sender, stream) = RequestBodyStream::channel();
        let producer = tokio::spawn(async move {
            assert!(sender.send(Bytes::from_static(b"part")).await);
            assert!(sender.send(Bytes::from_static(b"more")).await);
        });
        let error = match spool_request_body(
            Some(&stream),
            true,
            2,
            &config.temp_path,
            ClientBodyInFileOnly::Off,
            5,
        )
        .await
        {
            Ok(_) => panic!("request body should exceed the configured limit"),
            Err(error) => error,
        };
        producer.await.unwrap();

        assert_eq!(crate::http::request_body_error_status(&error), Some(413));
        assert!(
            !contains_generated_temp_file(&root),
            "overflowed request body must not leave a temporary file"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn truncated_file_backed_body_is_an_internal_error() {
        let root = test_root("truncated");
        let config = crate::config::ClientBodyConfig::new(root.clone());
        let (sender, stream) = RequestBodyStream::channel();
        let producer = tokio::spawn(async move {
            assert!(sender.send(Bytes::from_static(b"part")).await);
            assert!(sender.send(Bytes::from_static(b"more")).await);
        });
        let result = spool_request_body(
            Some(&stream),
            true,
            4,
            &config.temp_path,
            ClientBodyInFileOnly::Off,
            0,
        )
        .await
        .unwrap();
        producer.await.unwrap();

        let file = result.file.expect("spilled request body");
        std::fs::write(file.path(), b"part").unwrap();
        let error = RequestBodyStream::from_file(file.clone())
            .read_to_end(0)
            .await
            .expect_err("truncated request body file must fail");
        assert_eq!(crate::http::request_body_error_status(&error), Some(500));

        drop(file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn missing_file_backed_body_is_an_internal_error() {
        let root = test_root("missing");
        let config = crate::config::ClientBodyConfig::new(root.clone());
        let (sender, stream) = RequestBodyStream::channel();
        let producer = tokio::spawn(async move {
            assert!(sender.send(Bytes::from_static(b"part")).await);
        });
        let result = spool_request_body(
            Some(&stream),
            true,
            2,
            &config.temp_path,
            ClientBodyInFileOnly::Off,
            0,
        )
        .await
        .unwrap();
        producer.await.unwrap();

        let file = result.file.expect("spilled request body");
        std::fs::remove_file(file.path()).unwrap();
        let error = RequestBodyStream::from_file(file.clone())
            .read_to_end(0)
            .await
            .expect_err("missing request body file must fail");
        assert_eq!(crate::http::request_body_error_status(&error), Some(500));

        drop(file);
        let _ = std::fs::remove_dir_all(root);
    }

    async fn collect(stream: RequestBodyStream) -> Vec<u8> {
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await.unwrap() {
            body.extend_from_slice(&chunk);
        }
        body
    }

    fn test_root(label: &str) -> PathBuf {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "starling-body-spool-{label}-{}-{id}",
            std::process::id()
        ))
    }

    fn contains_generated_temp_file(path: &std::path::Path) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        entries.flatten().any(|entry| {
            let path = entry.path();
            path.is_dir() && contains_generated_temp_file(&path)
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(".starling-tmp-"))
        })
    }
}
