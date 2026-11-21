use super::*;

pub(in crate::config::compile) fn parse_proxy_pass(
    directive: &Directive,
) -> ConfigResult<ProxyPassConfig> {
    let raw = one_arg(directive)?.to_string();
    let Some(scheme_end) = raw.find("://") else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid URL prefix in \"proxy_pass\"",
        ));
    };
    let scheme = raw[..scheme_end].to_string();
    if scheme != "http" && scheme != "https" {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid URL prefix in \"proxy_pass\"",
        ));
    }
    let remainder = &raw[scheme_end + 3..];
    let (host, port, uri) = if let Some(socket) = remainder.strip_prefix("unix:") {
        let (path, uri) = match socket.find(":/") {
            Some(index) => (&socket[..index], Some(socket[index + 1..].to_string())),
            None => (socket, None),
        };
        if path.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid UNIX domain socket path in \"proxy_pass\"",
            ));
        }
        (format!("unix:{path}"), 0, uri)
    } else {
        let (authority, uri) = match remainder.find('/') {
            Some(index) => (&remainder[..index], Some(remainder[index..].to_string())),
            None => (remainder, None),
        };
        let (host, port) = parse_authority(authority, scheme == "https", &directive.location)?;
        (host, port, uri)
    };
    let upstream =
        if !host.contains('$') && port != 0 && port == if scheme == "https" { 443 } else { 80 } {
            Some(host.clone())
        } else {
            None
        };
    Ok(ProxyPassConfig {
        scheme,
        host,
        port,
        uri,
        upstream,
        raw,
    })
}

pub(in crate::config::compile) fn parse_authority(
    authority: &str,
    https: bool,
    location: &SourceLocation,
) -> ConfigResult<(String, u16)> {
    if authority.is_empty() {
        return Err(ConfigError::at(
            location.clone(),
            "no host in \"proxy_pass\" URL",
        ));
    }
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, rest)) = rest.split_once(']') else {
            return Err(ConfigError::at(
                location.clone(),
                "invalid IPv6 address in \"proxy_pass\" URL",
            ));
        };
        let port = match rest.strip_prefix(':') {
            Some(port) => parse_port(port, location)?,
            None if rest.is_empty() => {
                if https {
                    443
                } else {
                    80
                }
            }
            _ => {
                return Err(ConfigError::at(
                    location.clone(),
                    "invalid host in \"proxy_pass\" URL",
                ));
            }
        };
        return Ok((format!("[{host}]"), port));
    }
    if let Some((host, port)) = authority.rsplit_once(':')
        && !host.contains(':')
        && !port.contains('$')
    {
        return Ok((host.to_string(), parse_port(port, location)?));
    }
    Ok((authority.to_string(), if https { 443 } else { 80 }))
}
