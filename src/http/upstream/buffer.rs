use std::io;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;

use super::*;
use crate::config::{TempPathConfig, UpstreamBufferingConfig};

#[derive(Debug, Clone)]
pub struct UpstreamBodyBufferOptions {
    pub memory_limit: usize,
    pub busy_limit: usize,
    pub buffer_count: usize,
    pub buffer_size: usize,
    pub temp_path: Option<TempPathConfig>,
    pub max_temp_file_size: u64,
    pub write_size: usize,
}

impl UpstreamBodyBufferOptions {
    pub fn memory(limit: usize) -> Self {
        Self {
            memory_limit: limit,
            busy_limit: limit,
            buffer_count: 1,
            buffer_size: limit.max(1),
            temp_path: None,
            max_temp_file_size: 0,
            write_size: 16 * 1024,
        }
    }

    pub fn upstream(config: &UpstreamBufferingConfig) -> Self {
        Self::configured(config, config.enabled)
    }

    pub fn buffered(config: &UpstreamBufferingConfig) -> Self {
        Self::configured(config, true)
    }

    fn configured(config: &UpstreamBufferingConfig, enabled: bool) -> Self {
        if !enabled {
            return Self::memory(MAX_BODY_BYTES);
        }
        Self {
            memory_limit: config.total_buffers_size(),
            busy_limit: config.resolved_busy_buffers_size(),
            buffer_count: config.buffers.count,
            buffer_size: config.buffers.size,
            temp_path: Some(config.temp_path.clone()),
            max_temp_file_size: config.resolved_max_temp_file_size(),
            write_size: config.resolved_temp_file_write_size(),
        }
    }
}

pub struct BufferedUpstreamBody {
    pub body: ResponseBody,
    pub temp_file: Option<Arc<TempFileGuard>>,
}

pub(crate) struct BodyBuffer {
    options: UpstreamBodyBufferOptions,
    memory: Vec<u8>,
    file: Option<tokio::fs::File>,
    file_length: u64,
    guard: Option<Arc<TempFileGuard>>,
    length: u64,
}

impl BodyBuffer {
    pub(crate) fn new(options: UpstreamBodyBufferOptions) -> Self {
        Self {
            options,
            memory: Vec::new(),
            file: None,
            file_length: 0,
            guard: None,
            length: 0,
        }
    }

    pub(crate) async fn append(&mut self, data: &[u8]) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let data_len = u64::try_from(data.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "upstream body too large"))?;
        let total = self
            .length
            .checked_add(data_len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "upstream body too large"))?;
        let spill_capacity = if self.options.temp_path.is_some() {
            self.options.max_temp_file_size
        } else {
            0
        };
        let capacity = (self.options.memory_limit as u64).saturating_add(spill_capacity);
        if total > capacity {
            let message =
                if self.options.temp_path.is_some() && total > self.options.memory_limit as u64 {
                    "upstream response exceeds configured temp file size"
                } else {
                    "upstream response exceeds configured buffers"
                };
            return Err(io::Error::new(io::ErrorKind::InvalidData, message));
        }

        if self.file.is_some() {
            self.ensure_file_capacity(data_len)?;
            self.write_file(data).await?;
        } else {
            let memory_room = self.options.memory_limit.saturating_sub(self.memory.len());
            let in_memory = memory_room.min(data.len());
            self.memory.extend_from_slice(&data[..in_memory]);
            let spilled = &data[in_memory..];
            if !spilled.is_empty() {
                self.ensure_file_capacity(spilled.len() as u64)?;
                self.start_file().await?;
                self.write_file(spilled).await?;
            }
        }
        self.length = total;
        Ok(())
    }

    pub(crate) async fn finish(mut self) -> io::Result<BufferedUpstreamBody> {
        let Some(mut file) = self.file.take() else {
            return Ok(BufferedUpstreamBody {
                body: ResponseBody::Bytes(self.memory),
                temp_file: None,
            });
        };
        file.flush().await?;
        let body = if self.memory.is_empty() {
            ResponseBody::File {
                file,
                offset: 0,
                length: self.file_length,
            }
        } else {
            ResponseBody::FileWithPrefix {
                prefix: std::mem::take(&mut self.memory),
                file,
                offset: 0,
                length: self.file_length,
            }
        };
        Ok(BufferedUpstreamBody {
            body,
            temp_file: self.guard.take(),
        })
    }

    fn ensure_file_capacity(&self, additional: u64) -> io::Result<()> {
        if self.options.temp_path.is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream response exceeds configured buffers",
            ));
        }
        if self
            .file_length
            .checked_add(additional)
            .is_none_or(|length| length > self.options.max_temp_file_size)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream response exceeds configured temp file size",
            ));
        }
        Ok(())
    }

    async fn start_file(&mut self) -> io::Result<()> {
        let temp_path = self.options.temp_path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream response exceeds configured buffers",
            )
        })?;
        let (path, file) = create_temp_file(temp_path, "upstream body")?;
        self.file = Some(tokio::fs::File::from_std(file));
        self.guard = Some(Arc::new(TempFileGuard::new(path, true)));
        Ok(())
    }

    async fn write_file(&mut self, mut data: &[u8]) -> io::Result<()> {
        let file = self.file.as_mut().expect("temp file initialized");
        let write_size = self.options.write_size.max(1);
        while !data.is_empty() {
            let chunk_len = data.len().min(write_size);
            file.write_all(&data[..chunk_len]).await?;
            self.file_length = self
                .file_length
                .checked_add(chunk_len as u64)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "upstream response too large")
                })?;
            data = &data[chunk_len..];
        }
        Ok(())
    }
}
