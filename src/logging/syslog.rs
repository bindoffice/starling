use std::collections::HashMap;
use std::io;
use std::net::UdpSocket;
#[cfg(unix)]
use std::os::unix::net::UnixDatagram;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{SyslogConfig, SyslogServer};

static SYSLOG_PEERS: OnceLock<Mutex<HashMap<SyslogServer, SyslogPeer>>> = OnceLock::new();

enum SyslogPeer {
    Inet(UdpSocket),
    #[cfg(unix)]
    Unix(UnixDatagram),
}

pub fn send_syslog(config: &SyslogConfig, line: &str) {
    send_syslog_with_severity(config, config.severity, line);
}

pub fn send_syslog_with_severity(config: &SyslogConfig, severity: u8, line: &str) {
    let message = format_syslog_message(
        config,
        severity,
        line,
        &syslog_timestamp(SystemTime::now()),
        &hostname(),
    );
    let mut peers = syslog_peers()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !peers.contains_key(&config.server) {
        let Ok(peer) = SyslogPeer::connect(&config.server) else {
            return;
        };
        peers.insert(config.server.clone(), peer);
    }
    let result = peers
        .get_mut(&config.server)
        .is_some_and(|peer| peer.send(message.as_bytes()).is_ok());
    if !result {
        peers.remove(&config.server);
    }
}

impl SyslogPeer {
    fn connect(server: &SyslogServer) -> io::Result<Self> {
        match server {
            SyslogServer::Inet(address) => {
                let bind = if address.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                };
                let socket = UdpSocket::bind(bind)?;
                socket.set_nonblocking(true)?;
                socket.connect(address)?;
                Ok(Self::Inet(socket))
            }
            #[cfg(unix)]
            SyslogServer::Unix(path) => {
                let socket = UnixDatagram::unbound()?;
                socket.set_nonblocking(true)?;
                socket.connect(path)?;
                Ok(Self::Unix(socket))
            }
        }
    }

    fn send(&self, message: &[u8]) -> io::Result<usize> {
        match self {
            Self::Inet(socket) => socket.send(message),
            #[cfg(unix)]
            Self::Unix(socket) => socket.send(message),
        }
    }
}

fn syslog_peers() -> &'static Mutex<HashMap<SyslogServer, SyslogPeer>> {
    SYSLOG_PEERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn format_syslog_message(
    config: &SyslogConfig,
    severity: u8,
    line: &str,
    timestamp: &str,
    hostname: &str,
) -> String {
    let mut message = format!("<{}>{} ", config.priority_for(severity), timestamp);
    if !config.nohostname {
        message.push_str(hostname);
        message.push(' ');
    }
    message.push_str(&config.tag);
    message.push_str(": ");
    message.push_str(line.strip_suffix('\n').unwrap_or(line));
    message
}

fn syslog_timestamp(time: SystemTime) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // SAFETY: libc::tm consists of integer and pointer fields, so its all-zero
    // representation is valid.
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers reference live, correctly initialized storage for
    // the duration of the call.
    unsafe {
        libc::localtime_r(&seconds, &mut value);
    }
    let month = MONTHS
        .get(value.tm_mon.unsigned_abs() as usize)
        .copied()
        .unwrap_or("Jan");
    format!(
        "{month} {:2} {:02}:{:02}:{:02}",
        value.tm_mday, value.tm_hour, value.tm_min, value.tm_sec
    )
}

fn hostname() -> String {
    rustix::system::uname()
        .nodename()
        .to_string_lossy()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    fn config(nohostname: bool) -> SyslogConfig {
        SyslogConfig {
            server: SyslogServer::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 514)),
            facility: 16,
            severity: 4,
            tag: "starling".into(),
            nohostname,
        }
    }

    #[test]
    fn formats_rfc3164_message_without_the_trailing_linefeed() {
        let config = config(false);
        assert_eq!(
            format_syslog_message(
                &config,
                config.severity,
                "GET / HTTP/1.1 200\n",
                "Nov 20 12:34:56",
                "web-1"
            ),
            "<132>Nov 20 12:34:56 web-1 starling: GET / HTTP/1.1 200"
        );
    }

    #[test]
    fn omits_hostname_when_configured() {
        let config = config(true);
        assert_eq!(
            format_syslog_message(
                &config,
                config.severity,
                "GET / HTTP/1.1 200\n",
                "Nov 20 12:34:56",
                "web-1"
            ),
            "<132>Nov 20 12:34:56 starling: GET / HTTP/1.1 200"
        );
    }

    #[test]
    fn overrides_access_log_severity_for_error_logs() {
        assert_eq!(
            format_syslog_message(
                &config(true),
                3,
                "2026/11/20 12:34:56 [error] 1#0: failure\n",
                "Nov 20 12:34:56",
                "web-1"
            ),
            "<131>Nov 20 12:34:56 starling: 2026/11/20 12:34:56 [error] 1#0: failure"
        );
    }
}
