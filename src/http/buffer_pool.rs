use std::sync::{Mutex, OnceLock};

const POOL_BUFFER_SIZE: usize = 64 * 1024;
const MAX_BUFFERS: usize = 64;

static BUFFERS: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();

pub(crate) struct PooledBuffer {
    buffer: Option<Vec<u8>>,
}

impl PooledBuffer {
    pub(crate) fn acquire(min_len: usize) -> Self {
        if min_len <= POOL_BUFFER_SIZE {
            let buffer = BUFFERS
                .get_or_init(|| Mutex::new(Vec::new()))
                .lock()
                .ok()
                .and_then(|mut buffers| buffers.pop())
                .unwrap_or_else(|| vec![0; POOL_BUFFER_SIZE]);
            Self {
                buffer: Some(buffer),
            }
        } else {
            Self {
                buffer: Some(vec![0; min_len]),
            }
        }
    }
}

impl std::ops::Deref for PooledBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.buffer.as_deref().unwrap_or_default()
    }
}

impl std::ops::DerefMut for PooledBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buffer.as_deref_mut().unwrap_or_default()
    }
}

impl Drop for PooledBuffer {
    fn drop(&mut self) {
        let Some(buffer) = self.buffer.take() else {
            return;
        };
        if buffer.len() != POOL_BUFFER_SIZE {
            return;
        }
        if let Ok(mut buffers) = BUFFERS.get_or_init(|| Mutex::new(Vec::new())).lock()
            && buffers.len() < MAX_BUFFERS
        {
            buffers.push(buffer);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuses_released_file_buffers() {
        let first = PooledBuffer::acquire(1);
        let address = first.as_ptr();
        drop(first);

        let second = PooledBuffer::acquire(POOL_BUFFER_SIZE);
        assert_eq!(second.as_ptr(), address);
        assert_eq!(second.len(), POOL_BUFFER_SIZE);
    }

    #[test]
    fn oversized_buffers_are_not_returned_to_the_pool() {
        let buffer = PooledBuffer::acquire(POOL_BUFFER_SIZE + 1);
        assert_eq!(buffer.len(), POOL_BUFFER_SIZE + 1);
    }
}
