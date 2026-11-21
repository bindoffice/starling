use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ClientBodyInFileOnly {
    #[default]
    Off,
    On,
    Clean,
}

impl ClientBodyInFileOnly {
    pub fn remove_on_drop(self) -> bool {
        self != Self::On
    }

    pub fn enabled(self) -> bool {
        self != Self::Off
    }
}

pub type ClientBodyTempPath = TempPathConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientBodyConfig {
    pub buffer_size: usize,
    pub temp_path: ClientBodyTempPath,
    pub in_file_only: ClientBodyInFileOnly,
    pub in_single_buffer: bool,
}

impl ClientBodyConfig {
    pub fn new(temp_path: PathBuf) -> Self {
        Self {
            buffer_size: 2 * 4096,
            temp_path: ClientBodyTempPath::new(temp_path),
            in_file_only: ClientBodyInFileOnly::Off,
            in_single_buffer: false,
        }
    }
}
