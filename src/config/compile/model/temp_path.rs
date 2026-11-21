use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempPathConfig {
    pub path: PathBuf,
    pub levels: [usize; 3],
}

impl TempPathConfig {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            levels: [0; 3],
        }
    }
}
