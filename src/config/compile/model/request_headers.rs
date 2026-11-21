#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LargeClientHeaderBuffers {
    pub number: usize,
    pub size: usize,
}

impl Default for LargeClientHeaderBuffers {
    fn default() -> Self {
        Self {
            number: 4,
            size: 8 * 1024,
        }
    }
}

impl LargeClientHeaderBuffers {
    pub fn total_size(self, client_header_buffer_size: usize) -> Option<usize> {
        self.number
            .checked_mul(self.size)?
            .checked_add(client_header_buffer_size)
    }
}
