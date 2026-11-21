use super::*;

pub const UPSTREAM_BUFFER_PAGE_SIZE: usize = 4096;
pub const UPSTREAM_DEFAULT_BUFFER_COUNT: usize = 8;
pub const UPSTREAM_DEFAULT_MAX_TEMP_FILE_SIZE: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamBuffers {
    pub count: usize,
    pub size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamBufferingConfig {
    pub enabled: bool,
    pub buffer_size: usize,
    pub buffers: UpstreamBuffers,
    pub busy_buffers_size: Option<usize>,
    pub temp_path: TempPathConfig,
    pub max_temp_file_size: Option<u64>,
    pub temp_file_write_size: Option<usize>,
}

impl UpstreamBufferingConfig {
    pub fn new(temp_path: PathBuf) -> Self {
        Self {
            enabled: true,
            buffer_size: UPSTREAM_BUFFER_PAGE_SIZE,
            buffers: UpstreamBuffers {
                count: UPSTREAM_DEFAULT_BUFFER_COUNT,
                size: UPSTREAM_BUFFER_PAGE_SIZE,
            },
            busy_buffers_size: None,
            temp_path: TempPathConfig::new(temp_path),
            max_temp_file_size: None,
            temp_file_write_size: None,
        }
    }

    pub fn max_buffer_size(&self) -> usize {
        self.buffer_size.max(self.buffers.size)
    }

    pub fn resolved_busy_buffers_size(&self) -> usize {
        self.busy_buffers_size
            .unwrap_or_else(|| self.max_buffer_size().saturating_mul(2))
    }

    pub fn resolved_max_temp_file_size(&self) -> u64 {
        self.max_temp_file_size
            .unwrap_or(UPSTREAM_DEFAULT_MAX_TEMP_FILE_SIZE)
    }

    pub fn resolved_temp_file_write_size(&self) -> usize {
        self.temp_file_write_size
            .unwrap_or_else(|| self.max_buffer_size().saturating_mul(2))
    }

    pub fn total_buffers_size(&self) -> usize {
        self.buffers.count.saturating_mul(self.buffers.size)
    }
}
