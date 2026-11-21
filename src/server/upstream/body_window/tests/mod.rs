use super::*;
use crate::config::TempPathConfig;

mod basics;
mod busy_cap;
mod cancellation;
mod capacity;
mod permissions;

fn options(
    directory: PathBuf,
    memory: usize,
    temp: u64,
    write: usize,
) -> UpstreamBodyBufferOptions {
    UpstreamBodyBufferOptions {
        memory_limit: memory,
        busy_limit: usize::MAX,
        buffer_count: 1,
        buffer_size: memory.max(write).max(1),
        temp_path: Some(TempPathConfig::new(directory)),
        max_temp_file_size: temp,
        write_size: write,
    }
}

fn options_with_busy(
    directory: PathBuf,
    memory: usize,
    temp: u64,
    write: usize,
    busy: usize,
) -> UpstreamBodyBufferOptions {
    let mut options = options(directory, memory, temp, write);
    options.busy_limit = busy;
    options
}
