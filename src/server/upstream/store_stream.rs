use super::*;
use tokio::io::AsyncWriteExt;

pub(in crate::server) struct StoreTarget {
    pub(in crate::server) path: PathBuf,
    pub(in crate::server) access: u32,
    pub(in crate::server) modified: Option<SystemTime>,
    pub(in crate::server) expected_length: Option<u64>,
}

pub(in crate::server) fn attach(body: &mut ResponseBody, target: StoreTarget) -> io::Result<bool> {
    let stream = match std::mem::replace(body, ResponseBody::Empty) {
        ResponseBody::Stream(stream) => stream,
        replacement => {
            *body = replacement;
            return Ok(false);
        }
    };
    let expected_length = target.expected_length.or_else(|| stream.length());
    let StoreTarget {
        path,
        access,
        modified,
        ..
    } = target;
    let parent = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let temporary = temporary_path(&parent, "store");
    let prepare = async move {
        tokio::fs::create_dir_all(&parent).await?;
        let _ = remove_stale_temporary_files_in_directory(&parent, "store").await;
        let cleanup = TemporaryFileGuard::new(temporary.clone());
        let output = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .await?;
        Ok::<_, io::Error>(StoreTeeSink {
            output: Some(output),
            cleanup,
            temporary,
            path,
            access,
            modified,
            expected_length,
            written: 0,
        })
    };
    *body = ResponseBody::Stream(tee_response_stream(stream, prepare));
    Ok(true)
}

pub(in crate::server) async fn set_metadata(
    path: &Path,
    modified: Option<SystemTime>,
    access: u32,
) -> io::Result<()> {
    if let Some(modified) = modified {
        set_modified(path, modified).await?;
    }
    set_permissions(path, access).await
}

pub(in crate::server) fn modified_time(headers: &[Header]) -> Option<SystemTime> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("last-modified"))
        .and_then(|header| httpdate::parse_http_date(header.value.trim()).ok())
}

struct StoreTeeSink {
    output: Option<tokio::fs::File>,
    cleanup: TemporaryFileGuard,
    temporary: PathBuf,
    path: PathBuf,
    access: u32,
    modified: Option<SystemTime>,
    expected_length: Option<u64>,
    written: u64,
}

#[allow(clippy::manual_async_fn)]
impl TeeSink for StoreTeeSink {
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
                .expect("store tee output")
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
                drop(output);
                set_metadata(&self.temporary, self.modified, self.access).await?;
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

async fn set_modified(path: &Path, modified: SystemTime) -> io::Result<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        std::fs::File::options()
            .write(true)
            .open(path)?
            .set_times(std::fs::FileTimes::new().set_modified(modified))
    })
    .await
    .map_err(|error| io::Error::other(error.to_string()))?
}

#[cfg(unix)]
async fn set_permissions(path: &Path, access: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(access)).await
}

#[cfg(not(unix))]
async fn set_permissions(_path: &Path, _access: u32) -> io::Result<()> {
    Ok(())
}
