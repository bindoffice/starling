use super::*;

pub(in crate::config::compile) fn parse_grpc_pass(
    directive: &Directive,
) -> ConfigResult<GrpcPassConfig> {
    let raw = one_arg(directive)?.to_string();
    if raw.contains('$') {
        return Ok(GrpcPassConfig {
            tls: raw
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grpcs://")),
            dynamic: true,
            host: raw.clone(),
            port: 0,
            authority: raw.clone(),
            upstream: None,
            raw,
        });
    }

    let grpcs_prefix = raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grpcs://"));
    let grpc_prefix = raw
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grpc://"));
    let (target, tls) = if grpcs_prefix {
        (&raw[8..], true)
    } else if grpc_prefix {
        (&raw[7..], false)
    } else {
        (raw.as_str(), false)
    };
    if target.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no host in \"grpc_pass\" URL",
        ));
    }

    if let Some(path) = target.strip_prefix("unix:") {
        if path.is_empty() {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid UNIX domain socket path in \"grpc_pass\"",
            ));
        }
        return Ok(GrpcPassConfig {
            tls,
            dynamic: false,
            host: format!("unix:{path}"),
            port: 0,
            authority: "localhost".into(),
            upstream: None,
            raw,
        });
    }

    let authority = target
        .split_once('/')
        .map_or(target, |(authority, _)| authority);
    if authority.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no host in \"grpc_pass\" URL",
        ));
    }
    let (host, port, explicit_port) = parse_grpc_authority(authority, tls, &directive.location)?;
    let upstream = (!raw.contains("://") && !explicit_port).then(|| host.clone());
    Ok(GrpcPassConfig {
        tls,
        dynamic: false,
        authority: authority.to_string(),
        host,
        port,
        upstream,
        raw,
    })
}

fn parse_grpc_authority(
    authority: &str,
    tls: bool,
    location: &SourceLocation,
) -> ConfigResult<(String, u16, bool)> {
    let default_port = if tls { 443 } else { 80 };
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, rest)) = rest.split_once(']') else {
            return Err(ConfigError::at(
                location.clone(),
                "invalid IPv6 address in \"grpc_pass\" URL",
            ));
        };
        return match rest.strip_prefix(':') {
            Some(port) => Ok((format!("[{host}]"), parse_port(port, location)?, true)),
            None if rest.is_empty() => Ok((format!("[{host}]"), default_port, false)),
            _ => Err(ConfigError::at(
                location.clone(),
                "invalid host in \"grpc_pass\" URL",
            )),
        };
    }
    if let Some((host, port)) = authority.rsplit_once(':')
        && !host.contains(':')
    {
        return Ok((host.to_string(), parse_port(port, location)?, true));
    }
    if authority.contains(':') {
        return Err(ConfigError::at(
            location.clone(),
            "invalid host in \"grpc_pass\" URL",
        ));
    }
    Ok((authority.to_string(), default_port, false))
}
