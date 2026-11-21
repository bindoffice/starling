use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyslogConfig {
    pub server: SyslogServer,
    pub facility: u8,
    pub severity: u8,
    pub tag: String,
    pub nohostname: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SyslogServer {
    Inet(SocketAddr),
    Unix(PathBuf),
}

impl SyslogConfig {
    pub fn priority(&self) -> u8 {
        self.priority_for(self.severity)
    }

    pub fn priority_for(&self, severity: u8) -> u8 {
        self.facility * 8 + severity
    }
}
