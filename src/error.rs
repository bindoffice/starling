use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
    pub column: usize,
}

impl SourceLocation {
    pub fn new(file: impl Into<PathBuf>, line: usize, column: usize) -> Self {
        Self {
            file: file.into(),
            line,
            column,
        }
    }
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.file.display(), self.line, self.column)
    }
}

#[derive(Debug)]
pub struct ConfigError {
    pub location: Option<SourceLocation>,
    pub message: String,
}

impl ConfigError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            location: None,
            message: message.into(),
        }
    }

    pub fn at(location: SourceLocation, message: impl Into<String>) -> Self {
        Self {
            location: Some(location),
            message: message.into(),
        }
    }

    pub fn io(path: &Path, err: io::Error) -> Self {
        Self::new(format!("{}: {}", path.display(), err))
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.location {
            Some(location) => write!(f, "{}: {}", location, self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(value: io::Error) -> Self {
        Self::new(value.to_string())
    }
}

pub type ConfigResult<T> = Result<T, ConfigError>;
