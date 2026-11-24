use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum GzipHttpVersion {
    Http10,
    #[default]
    Http11,
}

impl GzipHttpVersion {
    pub(in crate::config::compile) fn parse(value: &str) -> Option<Self> {
        match value {
            "1.0" => Some(Self::Http10),
            "1.1" => Some(Self::Http11),
            _ => None,
        }
    }

    pub fn allows(self, request_version: &str) -> bool {
        match request_version {
            "HTTP/1.0" => self <= Self::Http10,
            "HTTP/1.1" | "HTTP/2.0" => self <= Self::Http11,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GzipDisableConfig {
    pub msie6: bool,
    pub regexes: Vec<Regex>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GzipProxiedConfig {
    pub any: bool,
    pub auth: bool,
    pub expired: bool,
    pub no_cache: bool,
    pub no_store: bool,
    pub private: bool,
    pub no_last_modified: bool,
    pub no_etag: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GzipStatic {
    #[default]
    Off,
    On,
    Always,
}
