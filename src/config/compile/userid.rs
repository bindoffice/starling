use super::*;

#[derive(Default)]
pub(super) struct UserIdSeen {
    enable: bool,
    service: bool,
    name: bool,
    domain: bool,
    path: bool,
    expires: bool,
    flags: bool,
    p3p: bool,
    mark: bool,
}

pub(super) fn is_userid_directive(name: &str) -> bool {
    matches!(
        name,
        "userid"
            | "userid_service"
            | "userid_name"
            | "userid_domain"
            | "userid_path"
            | "userid_expires"
            | "userid_flags"
            | "userid_p3p"
            | "userid_mark"
    )
}

pub(super) fn apply_userid_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut UserIdSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    match directive.name.as_str() {
        "userid" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.enable)?;
            context.userid.enable = match value {
                "off" => UserIdEnable::Off,
                "log" => UserIdEnable::Log,
                "v1" => UserIdEnable::V1,
                "on" => UserIdEnable::On,
                _ => return Err(invalid_userid_value(directive)),
            };
        }
        "userid_service" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.service)?;
            context.userid.service = Some(
                value
                    .parse::<u32>()
                    .map_err(|_| invalid_userid_value(directive))?,
            );
        }
        "userid_name" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.name)?;
            context.userid.name = value.to_string();
        }
        "userid_domain" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.domain)?;
            context.userid.domain = (value != "none").then(|| value.to_string());
        }
        "userid_path" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.path)?;
            context.userid.path = value.to_string();
        }
        "userid_expires" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.expires)?;
            context.userid.expires = match value {
                "off" => UserIdExpires::Off,
                "max" => UserIdExpires::Max,
                _ => {
                    let duration = parse_duration(value, &directive.location)
                        .map_err(|_| invalid_userid_value(directive))?;
                    if duration.is_zero() {
                        UserIdExpires::Off
                    } else {
                        UserIdExpires::Seconds(duration.as_secs())
                    }
                }
            };
        }
        "userid_flags" => {
            if !seen.flags {
                context.userid.flags = UserIdFlags::default();
                seen.flags = true;
            }
            if directive.args.is_empty() || directive.args.len() > 3 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"userid_flags\" directive",
                ));
            }
            for flag in &directive.args {
                match flag.to_ascii_lowercase().as_str() {
                    "off" => {}
                    "secure" => context.userid.flags.secure = true,
                    "httponly" => context.userid.flags.http_only = true,
                    "samesite=strict" => context.userid.flags.same_site_strict = true,
                    "samesite=lax" => context.userid.flags.same_site_lax = true,
                    "samesite=none" => context.userid.flags.same_site_none = true,
                    _ => return Err(invalid_userid_value(directive)),
                }
            }
        }
        "userid_p3p" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.p3p)?;
            context.userid.p3p = (value != "none").then(|| value.to_string());
        }
        "userid_mark" => {
            let value = one_arg(directive)?;
            reject_userid_duplicate(directive, &mut seen.mark)?;
            context.userid.mark = match value {
                "off" => None,
                _ if value == "="
                    || (value.len() == 1
                        && value
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)) =>
                {
                    Some(value.as_bytes()[0])
                }
                _ => return Err(invalid_userid_value(directive)),
            };
        }
        _ => unreachable!("checked by is_userid_directive"),
    }
    Ok(())
}

fn reject_userid_duplicate(directive: &Directive, configured: &mut bool) -> ConfigResult<()> {
    if *configured {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *configured = true;
    Ok(())
}

fn invalid_userid_value(directive: &Directive) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!("invalid value in \"{}\" directive", directive.name),
    )
}
