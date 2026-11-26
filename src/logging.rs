use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{LogDestination, LogLevel, LogTarget};

mod access;
mod file_cache;
mod format;
mod syslog;

pub use access::{append_access_log, flush_access_logs, reopen_access_logs};
pub use format::{render_log_format, render_log_format_bytes};
pub use syslog::{send_syslog, send_syslog_with_severity};

pub struct Logger {
    state: Mutex<LoggerState>,
}

struct LoggerState {
    sinks: Vec<LogSink>,
}

struct LogSink {
    destination: LogDestination,
    level: LogLevel,
    file: Option<File>,
}

impl Logger {
    pub fn new(targets: &[LogTarget], prefix: &Path) -> io::Result<Self> {
        Ok(Self {
            state: Mutex::new(LoggerState {
                sinks: build_sinks(targets, prefix)?,
            }),
        })
    }

    pub fn log(&self, level: LogLevel, message: impl AsRef<str>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let timestamp = nginx_timestamp(SystemTime::now());
        let line = format!(
            "{} [{}] {}#0: {}\n",
            timestamp,
            level.as_str(),
            std::process::id(),
            message.as_ref()
        );
        for sink in &mut state.sinks {
            if level < sink.level {
                continue;
            }
            match &sink.destination {
                LogDestination::Stderr => {
                    let _ = io::stderr().write_all(line.as_bytes());
                }
                LogDestination::File(_) => {
                    if let Some(file) = sink.file.as_mut() {
                        let _ = file.write_all(line.as_bytes());
                        let _ = file.flush();
                    }
                }
                LogDestination::Syslog(config) => {
                    send_syslog_with_severity(config, level.syslog_severity(), &line);
                }
            }
        }
    }

    pub fn enabled(&self, level: LogLevel) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sinks.iter().any(|sink| level >= sink.level)
    }

    pub fn reopen(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for sink in &mut state.sinks {
            let LogDestination::File(path) = &sink.destination else {
                continue;
            };
            if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
                sink.file = Some(file);
            }
        }
    }

    pub fn reconfigure(&self, targets: &[LogTarget], prefix: &Path) -> io::Result<()> {
        let sinks = build_sinks(targets, prefix)?;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.sinks = sinks;
        Ok(())
    }
}

fn build_sinks(targets: &[LogTarget], prefix: &Path) -> io::Result<Vec<LogSink>> {
    targets
        .iter()
        .map(|target| {
            let destination = match &target.destination {
                LogDestination::File(path) => {
                    let path = if path.is_absolute() {
                        path.clone()
                    } else {
                        prefix.join(path)
                    };
                    LogDestination::File(path)
                }
                LogDestination::Stderr => LogDestination::Stderr,
                LogDestination::Syslog(config) => LogDestination::Syslog(config.clone()),
            };
            let file = match &destination {
                LogDestination::File(path) => {
                    if let Some(parent) = path.parent()
                        && !parent.as_os_str().is_empty()
                    {
                        std::fs::create_dir_all(parent)?;
                    }
                    Some(OpenOptions::new().create(true).append(true).open(path)?)
                }
                LogDestination::Stderr | LogDestination::Syslog(_) => None,
            };
            Ok(LogSink {
                destination,
                level: target.level,
                file,
            })
        })
        .collect()
}

pub fn log_filter_allows(value: &str) -> bool {
    !value.is_empty() && value != "0"
}

pub fn nginx_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (year, month, day, hour, minute, second, _) = local_time_parts(seconds);
    format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}")
}

pub fn nginx_access_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (year, month, day, hour, minute, second, offset) = local_time_parts(seconds);
    let month_name = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][month.saturating_sub(1) as usize];
    format!(
        "{day:02}/{month_name}/{year:04}:{hour:02}:{minute:02}:{second:02} {}",
        format_offset(offset, false)
    )
}

pub fn nginx_iso_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let (year, month, day, hour, minute, second, offset) = local_time_parts(seconds);
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}{}",
        format_offset(offset, true)
    )
}

fn format_offset(offset: i64, colon: bool) -> String {
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    let hours = offset / 3600;
    let minutes = (offset % 3600) / 60;
    if colon {
        format!("{sign}{hours:02}:{minutes:02}")
    } else {
        format!("{sign}{hours:02}{minutes:02}")
    }
}

fn local_time_parts(timestamp: i64) -> (i32, u32, u32, u32, u32, u32, i64) {
    // SAFETY: libc::tm consists of integer and pointer fields, so its all-zero
    // representation is valid.
    let mut time: libc::tm = unsafe { std::mem::zeroed() };
    let raw = timestamp as libc::time_t;
    // SAFETY: both pointers reference live, correctly initialized storage for
    // the duration of the call.
    unsafe {
        libc::localtime_r(&raw, &mut time);
    }
    (
        time.tm_year + 1900,
        (time.tm_mon + 1) as u32,
        time.tm_mday as u32,
        time.tm_hour as u32,
        time.tm_min as u32,
        time.tm_sec as u32,
        time.tm_gmtoff as i64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn reconfigure_reopens_and_retargets_error_log() {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix =
            std::env::temp_dir().join(format!("starling-log-test-{}-{id}", std::process::id()));
        let first = LogTarget {
            destination: LogDestination::File("logs/first.log".into()),
            level: LogLevel::Error,
        };
        let second = LogTarget {
            destination: LogDestination::File("logs/second.log".into()),
            level: LogLevel::Notice,
        };
        let logger = Logger::new(std::slice::from_ref(&first), &prefix).unwrap();
        logger.log(LogLevel::Notice, "not written");
        logger
            .reconfigure(std::slice::from_ref(&second), &prefix)
            .unwrap();
        logger.log(LogLevel::Notice, "written after reload");
        drop(logger);

        let first_contents = std::fs::read_to_string(prefix.join("logs/first.log")).unwrap();
        let second_contents = std::fs::read_to_string(prefix.join("logs/second.log")).unwrap();
        assert!(first_contents.is_empty());
        assert!(second_contents.contains("written after reload"));
        let _ = std::fs::remove_dir_all(prefix);
    }

    #[test]
    fn writes_to_all_error_log_targets_at_their_configured_levels() {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let prefix =
            std::env::temp_dir().join(format!("starling-log-test-{}-{id}", std::process::id()));
        let targets = [
            LogTarget {
                destination: LogDestination::File("logs/all.log".into()),
                level: LogLevel::Info,
            },
            LogTarget {
                destination: LogDestination::File("logs/errors.log".into()),
                level: LogLevel::Error,
            },
        ];
        let logger = Logger::new(&targets, &prefix).unwrap();
        logger.log(LogLevel::Info, "informational message");
        logger.log(LogLevel::Error, "error message");
        drop(logger);

        let all = std::fs::read_to_string(prefix.join("logs/all.log")).unwrap();
        let errors = std::fs::read_to_string(prefix.join("logs/errors.log")).unwrap();
        assert!(all.contains("informational message"), "{all}");
        assert!(all.contains("error message"), "{all}");
        assert!(!errors.contains("informational message"), "{errors}");
        assert!(errors.contains("error message"), "{errors}");
        let _ = std::fs::remove_dir_all(prefix);
    }

    #[test]
    fn sends_error_logs_to_syslog_with_the_event_severity() {
        let socket = match std::net::UdpSocket::bind("127.0.0.1:0") {
            Ok(socket) => socket,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                eprintln!("skipping syslog error log test: binding sockets is not permitted");
                return;
            }
            Err(error) => panic!("bind syslog socket: {error}"),
        };
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let target = LogTarget {
            destination: LogDestination::Syslog(crate::config::SyslogConfig {
                server: crate::config::SyslogServer::Inet(socket.local_addr().unwrap()),
                facility: 16,
                severity: 6,
                tag: "starling".into(),
                nohostname: true,
            }),
            level: LogLevel::Info,
        };
        let logger = Logger::new(&[target], Path::new(".")).unwrap();
        logger.log(LogLevel::Error, "backend failed");

        let mut buffer = [0_u8; 2048];
        let length = socket.recv(&mut buffer).unwrap();
        let message = std::str::from_utf8(&buffer[..length]).unwrap();
        assert!(message.starts_with("<131>"), "{message}");
        assert!(message.contains(" [error] "), "{message}");
        assert!(message.contains(" starling: "), "{message}");
        assert!(message.ends_with("backend failed"), "{message}");
        assert!(!message.contains('\n'), "{message}");
    }
}
