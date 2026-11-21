use super::*;

pub(super) fn compile_limit_except(
    directive: &Directive,
    parent: &EffectiveConfig,
    prefix: &Path,
) -> ConfigResult<LimitExcept> {
    const METHODS: &[&str] = &[
        "GET",
        "HEAD",
        "POST",
        "PUT",
        "DELETE",
        "MKCOL",
        "COPY",
        "MOVE",
        "OPTIONS",
        "PROPFIND",
        "PROPPATCH",
        "LOCK",
        "UNLOCK",
        "PATCH",
    ];
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"limit_except\" directive",
        ));
    }
    let mut methods = Vec::new();
    for method in &directive.args {
        if !METHODS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(method))
        {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid method \"{method}\""),
            ));
        }
        methods.push(method.to_ascii_uppercase());
    }
    let mut access_rules = parent.access_rules.clone();
    let mut seen_access_rules = false;
    let mut proxy_pass = None;
    let mut auth_basic = parent.auth_basic.clone();
    let mut auth_basic_user_file = parent.auth_basic_user_file.clone();
    let mut access_log = parent.access_log.clone();
    let mut access_log_seen = false;
    for child in &directive.children {
        match child.name.as_str() {
            "allow" | "deny" => {
                validate_no_block(child)?;
                if !seen_access_rules {
                    access_rules.clear();
                    seen_access_rules = true;
                }
                access_rules.push(parse_access_rule(child)?);
            }
            "proxy_pass" => {
                validate_no_block(child)?;
                let proxy = parse_proxy_pass(child)?;
                if proxy.uri.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"proxy_pass\" cannot have URI part inside \"limit_except\" block",
                    ));
                }
                proxy_pass = Some(proxy);
            }
            "auth_basic" => {
                validate_no_block(child)?;
                let realm = one_arg(child)?;
                auth_basic = (realm != "off").then(|| realm.to_string());
            }
            "auth_basic_user_file" => {
                validate_no_block(child)?;
                auth_basic_user_file = Some(
                    resolve_path(prefix, one_arg(child)?)
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            "access_log" => {
                apply_access_log_directive(
                    child,
                    prefix,
                    Some("combined"),
                    &mut access_log,
                    &mut access_log_seen,
                )?;
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("\"{other}\" directive is not allowed here"),
                ));
            }
        }
    }
    Ok(LimitExcept {
        methods,
        access_rules,
        proxy_pass,
        auth_basic,
        auth_basic_user_file,
        access_log,
    })
}
