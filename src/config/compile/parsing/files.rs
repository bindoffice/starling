use super::*;

pub(in crate::config::compile) fn parse_password_file(
    prefix: &Path,
    directive: &Directive,
) -> ConfigResult<Vec<Vec<u8>>> {
    validate_no_block(directive)?;
    let path = resolve_path(prefix, one_arg(directive)?);
    let contents = std::fs::read(&path).map_err(|err| {
        ConfigError::at(
            directive.location.clone(),
            format!("open() \"{}\" failed ({err})", path.display()),
        )
    })?;
    let mut passwords = contents
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty())
        .map(<[u8]>::to_vec)
        .collect::<Vec<_>>();
    if passwords.is_empty() {
        passwords.push(Vec::new());
    }
    Ok(passwords)
}

pub(in crate::config::compile) fn parse_log_target(
    prefix: &Path,
    directive: &Directive,
) -> ConfigResult<LogTarget> {
    validate_no_block(directive)?;
    if directive.args.is_empty() || directive.args.len() > 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"error_log\" directive",
        ));
    }
    let destination = match directive.args[0].as_str() {
        "stderr" => LogDestination::Stderr,
        value if value.starts_with("syslog:") => {
            LogDestination::Syslog(parse_syslog_config(value, &directive.location)?)
        }
        value if value.starts_with("memory:") => {
            return Err(ConfigError::at(
                directive.location.clone(),
                "nginx was built without debug support",
            ));
        }
        value => LogDestination::File(resolve_path(prefix, value)),
    };
    let level = directive
        .args
        .get(1)
        .map(|value| {
            LogLevel::parse(value).ok_or_else(|| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid log level \"{value}\""),
                )
            })
        })
        .transpose()?
        .unwrap_or(LogLevel::Error);
    Ok(LogTarget { destination, level })
}
