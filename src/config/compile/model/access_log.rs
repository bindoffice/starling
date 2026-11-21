use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenLogFileCacheConfig {
    pub max: usize,
    pub inactive: Duration,
    pub min_uses: usize,
    pub valid: Duration,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogEscapeMode {
    #[default]
    Default,
    Json,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogFormat {
    pub template: String,
    pub escape: LogEscapeMode,
}

#[derive(Debug, Clone, Default)]
pub struct AccessLogConfig {
    pub off: bool,
    pub logs: Vec<AccessLogTarget>,
}

#[derive(Debug, Clone)]
pub struct AccessLogTarget {
    pub destination: AccessLogDestination,
    pub format_name: String,
    pub condition: Option<String>,
    pub buffer: Option<AccessLogBufferConfig>,
}

#[derive(Debug, Clone)]
pub enum AccessLogDestination {
    File(AccessLogPath),
    Syslog(SyslogConfig),
}

#[derive(Debug, Clone)]
pub enum AccessLogPath {
    Fixed(PathBuf),
    Dynamic(String),
}

impl AccessLogDestination {
    pub fn as_fixed_path(&self) -> Option<&Path> {
        match self {
            Self::File(path) => path.as_fixed_path(),
            Self::Syslog(_) => None,
        }
    }

    pub fn as_template(&self) -> Option<&str> {
        match self {
            Self::File(path) => path.as_template(),
            Self::Syslog(_) => None,
        }
    }

    pub fn as_syslog(&self) -> Option<&SyslogConfig> {
        match self {
            Self::Syslog(config) => Some(config),
            Self::File(_) => None,
        }
    }
}

impl AccessLogPath {
    pub fn as_fixed_path(&self) -> Option<&Path> {
        match self {
            Self::Fixed(path) => Some(path),
            Self::Dynamic(_) => None,
        }
    }

    pub fn as_template(&self) -> Option<&str> {
        match self {
            Self::Fixed(_) => None,
            Self::Dynamic(template) => Some(template),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessLogBufferConfig {
    pub size: usize,
    pub flush: Option<Duration>,
    pub gzip_level: Option<u32>,
}
