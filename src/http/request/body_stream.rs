use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, mpsc, watch};

const REQUEST_BODY_CHANNEL_CAPACITY: usize = 2;
const REQUEST_BODY_FILE_CHUNK_SIZE: usize = 64 * 1024;

pub struct RequestBodyStream {
    source: Arc<RequestBodySource>,
    file_reader: Arc<Mutex<FileReader>>,
}

enum RequestBodySource {
    Channel(ChannelBody),
    File(Arc<super::body_storage::RequestBodyFile>),
}

struct ChannelBody {
    receiver: Mutex<Option<mpsc::Receiver<io::Result<Bytes>>>>,
    h2_window: Option<watch::Sender<Option<u32>>>,
    ended: Arc<AtomicBool>,
}

enum FileReader {
    NotStarted,
    Open {
        reader: tokio::fs::File,
        remaining: u64,
    },
    Done,
}

pub(crate) struct RequestBodySender {
    sender: mpsc::Sender<io::Result<Bytes>>,
    ended: Arc<AtomicBool>,
}

#[derive(Debug)]
pub(crate) struct RequestBodyError {
    status: u16,
    message: String,
}

impl std::fmt::Display for RequestBodyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RequestBodyError {}

impl Clone for RequestBodyStream {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            // File-backed streams are replayable: every clone starts at byte
            // zero. Live channel clones continue to share the receiver.
            file_reader: Arc::new(Mutex::new(FileReader::NotStarted)),
        }
    }
}

impl RequestBodyStream {
    pub(crate) fn channel() -> (RequestBodySender, Self) {
        Self::channel_inner(None)
    }

    pub(crate) fn channel_with_h2_window() -> (RequestBodySender, Self, watch::Receiver<Option<u32>>)
    {
        let (window, receiver) = watch::channel(None);
        let (sender, stream) = Self::channel_inner(Some(window));
        (sender, stream, receiver)
    }

    fn channel_inner(h2_window: Option<watch::Sender<Option<u32>>>) -> (RequestBodySender, Self) {
        let (sender, receiver) = mpsc::channel(REQUEST_BODY_CHANNEL_CAPACITY);
        let ended = Arc::new(AtomicBool::new(false));
        (
            RequestBodySender {
                sender,
                ended: ended.clone(),
            },
            Self {
                source: Arc::new(RequestBodySource::Channel(ChannelBody {
                    receiver: Mutex::new(Some(receiver)),
                    h2_window,
                    ended,
                })),
                file_reader: Arc::new(Mutex::new(FileReader::NotStarted)),
            },
        )
    }

    pub(crate) fn from_file(file: Arc<super::body_storage::RequestBodyFile>) -> Self {
        Self {
            source: Arc::new(RequestBodySource::File(file)),
            file_reader: Arc::new(Mutex::new(FileReader::NotStarted)),
        }
    }

    pub fn is_file_backed(&self) -> bool {
        matches!(self.source.as_ref(), RequestBodySource::File(_))
    }

    pub fn file(&self) -> Option<Arc<super::body_storage::RequestBodyFile>> {
        match self.source.as_ref() {
            RequestBodySource::File(file) => Some(file.clone()),
            RequestBodySource::Channel(_) => None,
        }
    }

    pub fn file_len(&self) -> Option<u64> {
        self.file().map(|file| file.len())
    }

    pub async fn next(&self) -> io::Result<Option<Bytes>> {
        match self.source.as_ref() {
            RequestBodySource::Channel(channel) => {
                let mut receiver = channel.receiver.lock().await;
                let Some(channel) = receiver.as_mut() else {
                    return Ok(None);
                };
                let item = channel.recv().await.transpose()?;
                if item.is_none() {
                    *receiver = None;
                }
                Ok(item)
            }
            RequestBodySource::File(file) => {
                let mut state = self.file_reader.lock().await;
                if matches!(*state, FileReader::Done) {
                    return Ok(None);
                }
                if matches!(*state, FileReader::NotStarted) {
                    *state = FileReader::Open {
                        reader: file.open().await.map_err(request_body_file_error)?,
                        remaining: file.len(),
                    };
                }
                let FileReader::Open { reader, remaining } = &mut *state else {
                    return Ok(None);
                };
                if *remaining == 0 {
                    *state = FileReader::Done;
                    return Ok(None);
                }
                let read_size = (*remaining).min(REQUEST_BODY_FILE_CHUNK_SIZE as u64) as usize;
                let mut buffer = vec![0_u8; read_size];
                let read = reader
                    .read(&mut buffer)
                    .await
                    .map_err(request_body_file_error)?;
                if read == 0 {
                    *state = FileReader::Done;
                    return Err(request_body_status_error(
                        500,
                        "request body file ended before its recorded length",
                    ));
                }
                *remaining = remaining.saturating_sub(read as u64);
                buffer.truncate(read);
                Ok(Some(Bytes::from(buffer)))
            }
        }
    }

    pub async fn read_to_end(&self, limit: u64) -> io::Result<Vec<u8>> {
        let mut body = Vec::new();
        let mut total = 0_u64;
        while let Some(data) = self.next_limited(&mut total, limit).await? {
            let length = body.len().saturating_add(data.len());
            body.extend_from_slice(&data);
            debug_assert_eq!(length as u64, total);
        }
        Ok(body)
    }

    pub(crate) async fn next_limited(
        &self,
        total: &mut u64,
        limit: u64,
    ) -> io::Result<Option<Bytes>> {
        let data = self.next().await.map_err(classify_request_body_error)?;
        if let Some(data) = &data {
            *total = total
                .checked_add(data.len() as u64)
                .ok_or_else(request_body_too_large_error)?;
            if limit != 0 && *total > limit {
                return Err(request_body_too_large_error());
            }
        }
        Ok(data)
    }

    pub async fn close(&self) {
        match self.source.as_ref() {
            RequestBodySource::Channel(channel) => {
                channel.receiver.lock().await.take();
            }
            RequestBodySource::File(_) => {
                *self.file_reader.lock().await = FileReader::Done;
            }
        }
    }

    pub(crate) fn set_h2_window_target(&self, target: u32) {
        let RequestBodySource::Channel(channel) = self.source.as_ref() else {
            return;
        };
        if let Some(window) = &channel.h2_window {
            window.send_replace(Some(target));
        }
    }

    pub(crate) fn is_ended(&self) -> bool {
        match self.source.as_ref() {
            RequestBodySource::Channel(channel) => channel.ended.load(Ordering::Acquire),
            RequestBodySource::File(_) => true,
        }
    }
}

fn classify_request_body_error(error: io::Error) -> io::Error {
    if request_body_error_status(&error).is_some() {
        return error;
    }
    let status = if error.kind() == io::ErrorKind::TimedOut {
        408
    } else if matches!(
        error.kind(),
        io::ErrorKind::InvalidData
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    ) {
        400
    } else {
        500
    };
    request_body_status_error(status, &error.to_string())
}

pub(crate) fn request_body_too_large_error() -> io::Error {
    request_body_status_error(413, "request body too large")
}

pub(crate) fn malformed_request_body_error(message: &str) -> io::Error {
    request_body_status_error(400, message)
}

pub(crate) fn clone_request_body_error(error: &io::Error) -> io::Error {
    match request_body_error_status(error) {
        Some(status) => request_body_status_error(status, &error.to_string()),
        None => io::Error::new(error.kind(), error.to_string()),
    }
}

fn request_body_status_error(status: u16, message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        RequestBodyError {
            status,
            message: message.to_string(),
        },
    )
}

fn request_body_file_error(error: io::Error) -> io::Error {
    request_body_status_error(500, &format!("request body file read failed: {error}"))
}

pub(crate) fn request_body_error_status(error: &io::Error) -> Option<u16> {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<RequestBodyError>())
        .map(|error| error.status)
}

impl RequestBodySender {
    pub(crate) async fn send(&self, data: Bytes) -> bool {
        self.sender.send(Ok(data)).await.is_ok()
    }

    pub(crate) async fn fail(&self, error: io::Error) {
        let _ = self.sender.send(Err(error)).await;
    }

    pub(crate) fn finish(&self) {
        self.ended.store(true, Ordering::Release);
    }
}

impl std::fmt::Debug for RequestBodyStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RequestBodyStream")
            .field("file_backed", &self.is_file_backed())
            .finish_non_exhaustive()
    }
}
