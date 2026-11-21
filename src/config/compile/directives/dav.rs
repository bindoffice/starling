use super::*;

#[derive(Default)]
pub(super) struct DavSeen {
    methods: bool,
    create_full_put_path: bool,
    min_delete_depth: bool,
    access: bool,
}

pub(super) fn try_apply_dav_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut DavSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "dav_methods" => parse_methods(directive, context, seen)?,
        "create_full_put_path" => {
            reject_duplicate(
                directive,
                &mut seen.create_full_put_path,
                "create_full_put_path",
            )?;
            context.dav.create_full_put_path = parse_flag(directive, "create_full_put_path")?;
        }
        "min_delete_depth" => {
            reject_duplicate(directive, &mut seen.min_delete_depth, "min_delete_depth")?;
            validate_no_block(directive)?;
            context.dav.min_delete_depth =
                parse_usize(one_arg(directive)?, &directive.location, "min_delete_depth")?;
        }
        "dav_access" => {
            reject_duplicate(directive, &mut seen.access, "dav_access")?;
            context.dav.access = parse_file_access(directive, "dav_access")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_methods(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut DavSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"dav_methods\" directive",
        ));
    }
    if !seen.methods {
        context.dav.methods.clear();
        seen.methods = true;
    }
    for value in &directive.args {
        let method = match value.to_ascii_lowercase().as_str() {
            "off" => continue,
            "put" => DavMethod::Put,
            "delete" => DavMethod::Delete,
            "mkcol" => DavMethod::MkCol,
            "copy" => DavMethod::Copy,
            "move" => DavMethod::Move,
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value \"{value}\" in \"dav_methods\" directive"),
                ));
            }
        };
        if !context.dav.methods.contains(&method) {
            context.dav.methods.push(method);
        }
    }
    Ok(())
}

fn reject_duplicate(directive: &Directive, seen: &mut bool, name: &str) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{name}\" directive is duplicate"),
        ));
    }
    *seen = true;
    Ok(())
}
