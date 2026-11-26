use super::*;

const DEFAULT_SYSLOG_PORT: u16 = 514;
const DEFAULT_FACILITY: u8 = 23;
const DEFAULT_SEVERITY: u8 = 6;

const FACILITIES: [&str; 24] = [
    "kern", "user", "mail", "daemon", "auth", "intern", "lpr", "news", "uucp", "clock", "authpriv",
    "ftp", "ntp", "audit", "alert", "cron", "local0", "local1", "local2", "local3", "local4",
    "local5", "local6", "local7",
];

const SEVERITIES: [&str; 8] = [
    "emerg", "alert", "crit", "error", "warn", "notice", "info", "debug",
];

pub(super) fn parse_syslog_config(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<SyslogConfig> {
    let options = value.strip_prefix("syslog:").ok_or_else(|| {
        ConfigError::at(
            location.clone(),
            format!("invalid syslog destination \"{value}\""),
        )
    })?;
    let mut server = None;
    let mut facility = None;
    let mut severity = None;
    let mut tag = None;
    let mut nohostname = false;

    for option in options.split(',') {
        if let Some(value) = option.strip_prefix("server=") {
            if server.is_some() {
                return Err(ConfigError::at(
                    location.clone(),
                    "duplicate syslog \"server\"",
                ));
            }
            server = Some(parse_syslog_server(value).map_err(|error| {
                ConfigError::at(
                    location.clone(),
                    format!("{error} in syslog server \"{value}\""),
                )
            })?);
        } else if let Some(value) = option.strip_prefix("facility=") {
            if facility.is_some() {
                return Err(ConfigError::at(
                    location.clone(),
                    "duplicate syslog \"facility\"",
                ));
            }
            facility = Some(
                FACILITIES
                    .iter()
                    .position(|name| *name == value)
                    .map(|index| index as u8)
                    .ok_or_else(|| {
                        ConfigError::at(
                            location.clone(),
                            format!("unknown syslog facility \"{value}\""),
                        )
                    })?,
            );
        } else if let Some(value) = option.strip_prefix("severity=") {
            if severity.is_some() {
                return Err(ConfigError::at(
                    location.clone(),
                    "duplicate syslog \"severity\"",
                ));
            }
            severity = Some(
                SEVERITIES
                    .iter()
                    .position(|name| *name == value)
                    .map(|index| index as u8)
                    .ok_or_else(|| {
                        ConfigError::at(
                            location.clone(),
                            format!("unknown syslog severity \"{value}\""),
                        )
                    })?,
            );
        } else if let Some(value) = option.strip_prefix("tag=") {
            if tag.is_some() {
                return Err(ConfigError::at(
                    location.clone(),
                    "duplicate syslog \"tag\"",
                ));
            }
            validate_syslog_tag(value, location)?;
            tag = Some(value.to_string());
        } else if option == "nohostname" {
            nohostname = true;
        } else {
            return Err(ConfigError::at(
                location.clone(),
                format!("unknown syslog parameter \"{option}\""),
            ));
        }
    }

    let server =
        server.ok_or_else(|| ConfigError::at(location.clone(), "no syslog server specified"))?;
    Ok(SyslogConfig {
        server,
        facility: facility.unwrap_or(DEFAULT_FACILITY),
        severity: severity.unwrap_or(DEFAULT_SEVERITY),
        tag: tag.unwrap_or_else(|| "starling".into()),
        nohostname,
    })
}

fn validate_syslog_tag(value: &str, location: &SourceLocation) -> ConfigResult<()> {
    if value.len() > 32 {
        return Err(ConfigError::at(
            location.clone(),
            "syslog tag length exceeds 32",
        ));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ConfigError::at(
            location.clone(),
            "syslog \"tag\" only allows alphanumeric characters and underscore",
        ));
    }
    Ok(())
}

fn parse_syslog_server(value: &str) -> Result<SyslogServer, String> {
    if value
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("unix:"))
    {
        let path = &value[5..];
        if path.is_empty() {
            return Err("no path in the unix domain socket".into());
        }
        #[cfg(not(unix))]
        {
            return Err("unix domain sockets are unsupported on this platform".into());
        }
        #[cfg(unix)]
        {
            let max_path =
                std::mem::size_of::<libc::sockaddr_un>() - std::mem::size_of::<libc::sa_family_t>();
            if path.len() + 1 > max_path {
                return Err("too long path in the unix domain socket".into());
            }
            return Ok(SyslogServer::Unix(PathBuf::from(path)));
        }
    }

    let bracketed = value.starts_with('[');
    let (host, port) = if let Some(value) = value.strip_prefix('[') {
        let (host, suffix) = value.split_once(']').ok_or("invalid host")?;
        let port = if suffix.is_empty() {
            DEFAULT_SYSLOG_PORT
        } else {
            parse_syslog_port(suffix.strip_prefix(':').ok_or("invalid host")?)?
        };
        (host, port)
    } else {
        let (host, port) = match value.split_once(':') {
            Some((host, port)) => (host, parse_syslog_port(port)?),
            None => (value, DEFAULT_SYSLOG_PORT),
        };
        (host, port)
    };

    if host.is_empty() {
        return Err("no host".into());
    }
    if host.contains(['/', '?']) {
        return Err("invalid host".into());
    }
    if bracketed {
        let address = host
            .parse::<IpAddr>()
            .map_err(|_| "invalid IPv6 address".to_string())?;
        if !address.is_ipv6() {
            return Err("invalid IPv6 address".into());
        }
        return Ok(SyslogServer::Inet(SocketAddr::new(address, port)));
    }
    if let Ok(address) = host.parse::<IpAddr>() {
        return Ok(SyslogServer::Inet(SocketAddr::new(address, port)));
    }
    if host.contains(':') {
        return Err("invalid host".into());
    }
    (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .next()
        .map(SyslogServer::Inet)
        .ok_or_else(|| "host not found".into())
}

fn parse_syslog_port(value: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "invalid port".into())
}
