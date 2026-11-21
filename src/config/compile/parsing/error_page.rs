use super::*;

pub(in crate::config::compile) fn parse_error_page(
    directive: &Directive,
) -> ConfigResult<ErrorPage> {
    if directive.args.len() < 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"error_page\" directive",
        ));
    }
    let uri = directive.args.last().unwrap().clone();
    let mut statuses = Vec::new();
    let mut overwrite = ErrorPageOverwrite::Preserve;
    let mut overwrite_seen = false;
    for value in &directive.args[..directive.args.len() - 1] {
        if let Some(value) = value.strip_prefix('=') {
            if overwrite_seen {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "duplicate parameter \"=\" in \"error_page\" directive",
                ));
            }
            overwrite_seen = true;
            overwrite = if value.is_empty() {
                ErrorPageOverwrite::Response
            } else {
                ErrorPageOverwrite::Status(value.parse::<u16>().map_err(|_| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid status code \"{value}\""),
                    )
                })?)
            };
        } else {
            let status = value.parse::<u16>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid status code \"{value}\""),
                )
            })?;
            if !(300..=599).contains(&status) || status == 499 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid status code \"{value}\""),
                ));
            }
            statuses.push(status);
        }
    }
    if statuses.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "no status code in \"error_page\" directive",
        ));
    }
    Ok(ErrorPage {
        statuses,
        overwrite,
        uri,
    })
}
