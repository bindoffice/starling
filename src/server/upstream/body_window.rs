use std::collections::VecDeque;
use std::io;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use bytes::{Bytes, BytesMut};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};
use tokio::sync::{Mutex, Notify};

use super::*;
use crate::http::{UpstreamBodyBufferOptions, create_temp_file};

mod capacity;
mod stream;
use capacity::{CapacityState, FileExtent, mark_busy, release_file_extent};
pub(super) use stream::spawn_windowed_body;

pub(super) struct BodyWindow {
    options: UpstreamBodyBufferOptions,
    state: Mutex<WindowState>,
    capacity: Arc<StdMutex<CapacityState>>,
    changed: Arc<Notify>,
    #[cfg(test)]
    spill_write_barrier: Option<Arc<TestSpillWriteBarrier>>,
    #[cfg(test)]
    spill_write_failure_after: AtomicUsize,
}

struct WindowState {
    entries: VecDeque<WindowEntry>,
    spill: Option<SpillFile>,
    done: bool,
    error: Option<io::Error>,
    aborted: bool,
}

enum WindowEntry {
    Memory(BytesMut),
    File { offset: u64, length: usize },
    Trailers(Vec<Header>),
}

struct SpillFile {
    file: tokio::fs::File,
    _guard: Arc<TempFileGuard>,
}

#[cfg(test)]
#[derive(Default)]
struct TestSpillWriteBarrier {
    entered: Notify,
    release: Notify,
}

enum WindowItem {
    Data(Bytes),
    Trailers(Vec<Header>),
}

struct GuardedData {
    data: Bytes,
    capacity: Arc<StdMutex<CapacityState>>,
    changed: Arc<Notify>,
    memory: usize,
    busy: usize,
    file: Option<FileExtent>,
}

impl AsRef<[u8]> for GuardedData {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for GuardedData {
    fn drop(&mut self) {
        let mut capacity = self.capacity.lock().expect("body window capacity");
        capacity.memory_bytes = capacity.memory_bytes.saturating_sub(self.memory);
        capacity.busy_bytes = capacity.busy_bytes.saturating_sub(self.busy);
        if let Some(file) = self.file {
            release_file_extent(&mut capacity, file);
        }
        drop(capacity);
        self.changed.notify_waiters();
    }
}

impl BodyWindow {
    pub(super) fn new(options: UpstreamBodyBufferOptions) -> Self {
        Self {
            options,
            state: Mutex::new(WindowState {
                entries: VecDeque::new(),
                spill: None,
                done: false,
                error: None,
                aborted: false,
            }),
            capacity: Arc::new(StdMutex::new(CapacityState::new())),
            changed: Arc::new(Notify::new()),
            #[cfg(test)]
            spill_write_barrier: None,
            #[cfg(test)]
            spill_write_failure_after: AtomicUsize::new(usize::MAX),
        }
    }

    pub(super) async fn push_data(&self, data: Bytes) -> io::Result<()> {
        let mut offset = 0;
        while offset < data.len() {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut state = self.state.lock().await;
            if state.aborted {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "downstream body stream closed",
                ));
            }

            let memory_room = {
                let capacity = self.capacity.lock().expect("body window capacity");
                self.options
                    .memory_limit
                    .saturating_sub(capacity.memory_bytes)
            };
            if memory_room > 0 {
                if let Some(WindowEntry::Memory(buffer)) = state.entries.back_mut() {
                    let room = self.options.buffer_size.saturating_sub(buffer.len());
                    if room > 0 {
                        let length = room.min(memory_room).min(data.len() - offset);
                        buffer.extend_from_slice(&data[offset..offset + length]);
                        self.capacity
                            .lock()
                            .expect("body window capacity")
                            .memory_bytes += length;
                        drop(state);
                        self.changed.notify_waiters();
                        offset += length;
                        continue;
                    }
                }
                let length = memory_room
                    .min(data.len() - offset)
                    .min(self.options.buffer_size.max(1));
                let mut buffer = BytesMut::with_capacity(length);
                buffer.extend_from_slice(&data[offset..offset + length]);
                self.capacity
                    .lock()
                    .expect("body window capacity")
                    .memory_bytes += length;
                state.entries.push_back(WindowEntry::Memory(buffer));
                drop(state);
                self.changed.notify_waiters();
                offset += length;
                continue;
            }

            let temp_room = {
                let capacity = self.capacity.lock().expect("body window capacity");
                self.options
                    .max_temp_file_size
                    .saturating_sub(capacity.file_buffered)
            };
            if self.options.temp_path.is_some() && temp_room > 0 {
                let allocation = self
                    .capacity
                    .lock()
                    .expect("body window capacity")
                    .reserve_file_extent(temp_room, data.len() - offset, &self.options);
                let length = allocation.length;
                let file = match prepare_spill(&self.options, &mut state).await {
                    Ok(file) => file,
                    Err(error) => {
                        release_file_extent(
                            &mut self.capacity.lock().expect("body window capacity"),
                            FileExtent {
                                offset: allocation.offset,
                                length: allocation.length,
                            },
                        );
                        return Err(error);
                    }
                };
                drop(state);
                #[cfg(test)]
                if let Some(barrier) = &self.spill_write_barrier {
                    barrier.entered.notify_one();
                    barrier.release.notified().await;
                }
                #[cfg(test)]
                let written = match self.injected_spill_write_error() {
                    Some(error) => Err(error),
                    None => {
                        write_spill_file(file, allocation.offset, &data[offset..offset + length])
                            .await
                    }
                };
                #[cfg(not(test))]
                let written =
                    write_spill_file(file, allocation.offset, &data[offset..offset + length]).await;
                state = self.state.lock().await;
                if state.aborted {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "downstream body stream closed",
                    ));
                }
                if let Err(error) = written {
                    release_file_extent(
                        &mut self.capacity.lock().expect("body window capacity"),
                        FileExtent {
                            offset: allocation.offset,
                            length: allocation.length,
                        },
                    );
                    // Keep the existing spill file alive so file extents that
                    // were queued before this write can still be drained.
                    return Err(error);
                }
                state.entries.push_back(WindowEntry::File {
                    offset: allocation.offset,
                    length: allocation.length,
                });
                drop(state);
                self.changed.notify_waiters();
                offset += length;
                continue;
            }

            drop(state);
            notified.await;
        }
        Ok(())
    }

    pub(super) async fn push_trailers(&self, trailers: Vec<Header>) -> io::Result<()> {
        let mut state = self.state.lock().await;
        if state.aborted {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "downstream body stream closed",
            ));
        }
        state.entries.push_back(WindowEntry::Trailers(trailers));
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }

    pub(super) async fn finish(&self) {
        let mut state = self.state.lock().await;
        state.done = true;
        drop(state);
        self.changed.notify_waiters();
    }

    pub(super) async fn fail(&self, error: io::Error) {
        let mut state = self.state.lock().await;
        state.error = Some(error);
        state.done = true;
        drop(state);
        self.changed.notify_waiters();
    }

    pub(super) async fn abort(&self) {
        let mut state = self.state.lock().await;
        state.aborted = true;
        state.entries.clear();
        state.spill = None;
        state.error = None;
        drop(state);
        *self.capacity.lock().expect("body window capacity") = CapacityState::new();
        self.changed.notify_waiters();
    }

    async fn pop(&self) -> io::Result<Option<WindowItem>> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut state = self.state.lock().await;
            if state.aborted {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "downstream body stream closed",
                ));
            }
            let busy = self
                .capacity
                .lock()
                .expect("body window capacity")
                .busy_bytes;
            let busy_room = if self.options.busy_limit == 0 || self.options.busy_limit == usize::MAX
            {
                usize::MAX
            } else {
                self.options.busy_limit.saturating_sub(busy)
            };
            if self.options.busy_limit != 0
                && busy >= self.options.busy_limit
                && matches!(
                    state.entries.front(),
                    Some(WindowEntry::Memory(_) | WindowEntry::File { .. })
                )
            {
                drop(state);
                notified.await;
                continue;
            }
            let buffer_limit = self.options.buffer_size.max(1);
            let chunk_room = busy_room.min(buffer_limit);
            if let Some(entry) = state.entries.pop_front() {
                let entry = match entry {
                    WindowEntry::Memory(mut data) if data.len() > chunk_room => {
                        let remaining = data.split_off(chunk_room);
                        state.entries.push_front(WindowEntry::Memory(remaining));
                        WindowEntry::Memory(data)
                    }
                    WindowEntry::File { offset, length } if length > chunk_room => {
                        state.entries.push_front(WindowEntry::File {
                            offset: offset.saturating_add(chunk_room as u64),
                            length: length - chunk_room,
                        });
                        WindowEntry::File {
                            offset,
                            length: chunk_room,
                        }
                    }
                    entry => entry,
                };
                let item = read_entry(&mut state, &self.capacity, &self.changed, entry).await?;
                drop(state);
                self.changed.notify_waiters();
                return Ok(Some(item));
            }
            if let Some(error) = state.error.take() {
                return Err(error);
            }
            if state.done {
                return Ok(None);
            }
            drop(state);
            notified.await;
        }
    }

    #[cfg(test)]
    pub(super) async fn spill_path(&self) -> Option<PathBuf> {
        self.state
            .lock()
            .await
            .spill
            .as_ref()
            .map(|spill| spill._guard.path().to_path_buf())
    }

    #[cfg(test)]
    fn injected_spill_write_error(&self) -> Option<io::Error> {
        let remaining = self.spill_write_failure_after.load(Ordering::Relaxed);
        if remaining == usize::MAX {
            return None;
        }
        if remaining == 0 {
            self.spill_write_failure_after
                .store(usize::MAX, Ordering::Relaxed);
            return Some(io::Error::other("injected spill write failure"));
        }
        self.spill_write_failure_after
            .store(remaining - 1, Ordering::Relaxed);
        None
    }
}

async fn prepare_spill(
    options: &UpstreamBodyBufferOptions,
    state: &mut WindowState,
) -> io::Result<tokio::fs::File> {
    if state.spill.is_none() {
        let temp_path = options.temp_path.as_ref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "upstream response exceeds configured buffers",
            )
        })?;
        let (path, file) = create_temp_file(temp_path, "buffered upstream body")?;
        state.spill = Some(SpillFile {
            file: tokio::fs::File::from_std(file),
            _guard: Arc::new(TempFileGuard::new(path, true)),
        });
    }
    let spill = state.spill.as_mut().expect("spill file initialized");
    // The caller writes after releasing the window lock. Each write seeks
    // explicitly; reads below use a separate handle with its own cursor.
    spill.file.try_clone().await
}

async fn write_spill_file(mut file: tokio::fs::File, offset: u64, data: &[u8]) -> io::Result<()> {
    async {
        file.seek(SeekFrom::Start(offset)).await?;
        file.write_all(data).await
    }
    .await
}

async fn read_entry(
    state: &mut WindowState,
    capacity: &Arc<StdMutex<CapacityState>>,
    changed: &Arc<Notify>,
    entry: WindowEntry,
) -> io::Result<WindowItem> {
    match entry {
        WindowEntry::Memory(data) => {
            let data = data.freeze();
            let busy = data.len();
            mark_busy(capacity, busy);
            Ok(WindowItem::Data(Bytes::from_owner(GuardedData {
                memory: data.len(),
                busy,
                data,
                capacity: capacity.clone(),
                changed: changed.clone(),
                file: None,
            })))
        }
        WindowEntry::File { offset, length } => {
            let spill = state.spill.as_mut().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "upstream temp file is missing")
            })?;
            // A separate open keeps the reader's cursor independent from a
            // spill write that may still be in progress.
            let mut file = tokio::fs::File::open(spill._guard.path()).await?;
            file.seek(SeekFrom::Start(offset)).await?;
            let mut data = vec![0_u8; length];
            file.read_exact(&mut data).await?;
            mark_busy(capacity, length);
            Ok(WindowItem::Data(Bytes::from_owner(GuardedData {
                data: Bytes::from(data),
                capacity: capacity.clone(),
                changed: changed.clone(),
                memory: 0,
                busy: length,
                file: Some(FileExtent { offset, length }),
            })))
        }
        WindowEntry::Trailers(trailers) => Ok(WindowItem::Trailers(trailers)),
    }
}

#[cfg(test)]
mod tests;
