#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsiConfig {
    pub enabled: bool,
    pub silent_errors: bool,
    pub ignore_recycled_buffers: bool,
    pub last_modified: bool,
    pub types: Vec<String>,
    pub min_file_chunk: u64,
    pub value_length: u64,
}

impl Default for SsiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            silent_errors: false,
            ignore_recycled_buffers: false,
            last_modified: false,
            types: vec!["text/html".into()],
            min_file_chunk: 1024,
            value_length: 255,
        }
    }
}
