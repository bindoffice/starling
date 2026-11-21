use std::io;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::config::{ClientBodyInFileOnly, ClientBodyTempPath};
use crate::http::{TempFileGuard, create_temp_file};

#[derive(Debug)]
pub struct RequestBodyFile {
    guard: TempFileGuard,
    length: u64,
}

impl RequestBodyFile {
    pub(crate) fn new(guard: TempFileGuard, length: u64) -> Self {
        Self { guard, length }
    }

    pub fn path(&self) -> &Path {
        self.guard.path()
    }

    pub fn len(&self) -> u64 {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub async fn open(&self) -> io::Result<tokio::fs::File> {
        tokio::fs::File::open(self.path()).await
    }
}

pub fn store_request_body(
    body: &[u8],
    body_declared: bool,
    buffer_size: usize,
    temp_path: &ClientBodyTempPath,
    file_only: ClientBodyInFileOnly,
) -> io::Result<Option<Arc<RequestBodyFile>>> {
    let needs_file = body.len() > buffer_size || (file_only.enabled() && body_declared);
    if !body_declared || !needs_file {
        return Ok(None);
    }

    let (path, mut file) = create_temp_file(temp_path, "request body")?;
    let result = (|| {
        if let Err(err) = file.write_all(body) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(err);
        }
        if let Err(err) = file.flush() {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(err);
        }
        Ok(())
    })();
    match result {
        Ok(()) => Ok(Some(Arc::new(RequestBodyFile::new(
            TempFileGuard::new(path, file_only.remove_on_drop()),
            body.len() as u64,
        )))),
        Err(err) => Err(err),
    }
}
