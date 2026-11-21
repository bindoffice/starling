use std::collections::VecDeque;
use std::sync::{Arc, Mutex as StdMutex};

use crate::http::UpstreamBodyBufferOptions;

pub(super) struct CapacityState {
    pub(super) memory_bytes: usize,
    pub(super) file_buffered: u64,
    pub(super) file_size: u64,
    pub(super) busy_bytes: usize,
    free_file_extents: VecDeque<FileExtent>,
}

impl CapacityState {
    pub(super) fn new() -> Self {
        Self {
            memory_bytes: 0,
            file_buffered: 0,
            file_size: 0,
            busy_bytes: 0,
            free_file_extents: VecDeque::new(),
        }
    }

    pub(super) fn reserve_file_extent(
        &mut self,
        temp_room: u64,
        remaining: usize,
        options: &UpstreamBodyBufferOptions,
    ) -> FileAllocation {
        let requested = usize::try_from(temp_room)
            .unwrap_or(usize::MAX)
            .min(remaining)
            .min(file_chunk_limit(options));
        let offset;
        let length;
        if let Some(extent) = self.free_file_extents.front() {
            // A released allocation can be smaller than the next requested write.
            // Consume only this extent now; the caller will continue with the rest.
            offset = extent.offset;
            length = requested.min(extent.length);
            let extent = self.free_file_extents.front_mut().expect("free extent");
            extent.offset = extent.offset.saturating_add(length as u64);
            extent.length -= length;
            if extent.length == 0 {
                self.free_file_extents.pop_front();
            }
        } else {
            offset = self.file_size;
            length = requested;
            self.file_size = self.file_size.saturating_add(length as u64);
        }
        self.file_buffered = self.file_buffered.saturating_add(length as u64);
        FileAllocation { offset, length }
    }
}

#[derive(Clone, Copy)]
pub(super) struct FileExtent {
    pub(super) offset: u64,
    pub(super) length: usize,
}

pub(super) struct FileAllocation {
    pub(super) offset: u64,
    pub(super) length: usize,
}

pub(super) fn mark_busy(capacity: &Arc<StdMutex<CapacityState>>, amount: usize) {
    let mut capacity = capacity.lock().expect("body window capacity");
    capacity.busy_bytes = capacity.busy_bytes.saturating_add(amount);
}

pub(super) fn release_file_extent(capacity: &mut CapacityState, extent: FileExtent) {
    capacity.file_buffered = capacity.file_buffered.saturating_sub(extent.length as u64);
    let insert_at = capacity
        .free_file_extents
        .iter()
        .position(|current| current.offset > extent.offset)
        .unwrap_or(capacity.free_file_extents.len());
    capacity.free_file_extents.insert(insert_at, extent);
    coalesce_free_file_extents(&mut capacity.free_file_extents);
}

pub(super) fn file_chunk_limit(options: &UpstreamBodyBufferOptions) -> usize {
    let write_size = options.write_size.max(1);
    if options.busy_limit == 0 {
        write_size
    } else {
        write_size.min(options.busy_limit)
    }
}

fn coalesce_free_file_extents(extents: &mut VecDeque<FileExtent>) {
    let mut index = 0;
    while index + 1 < extents.len() {
        let current_end = extents[index]
            .offset
            .saturating_add(extents[index].length as u64);
        if current_end == extents[index + 1].offset {
            let next_length = extents[index + 1].length;
            extents[index].length = extents[index].length.saturating_add(next_length);
            extents.remove(index + 1);
        } else {
            index += 1;
        }
    }
}
