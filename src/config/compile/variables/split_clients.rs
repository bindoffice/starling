use super::*;

pub(in crate::config::compile) fn compile_split_clients(
    directive: &Directive,
) -> ConfigResult<SplitClientsVariable> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"split_clients\" directive",
        ));
    }
    let source = directive.args[0].clone();
    let name = parse_variable_name(&directive.args[1], &directive.location, "split_clients")?;
    let mut raw_parts = Vec::new();
    for child in &directive.children {
        validate_no_block(child)?;
        if child.args.len() != 1 {
            return Err(ConfigError::at(
                child.location.clone(),
                "invalid number of the split_clients parameters",
            ));
        }
        let percent = if child.name == "*" {
            0
        } else {
            parse_split_percent(&child.name).ok_or_else(|| {
                ConfigError::at(
                    child.location.clone(),
                    format!("invalid percent value \"{}\"", child.name),
                )
            })?
        };
        raw_parts.push((percent, child.args[0].clone()));
    }

    let mut parts = Vec::with_capacity(raw_parts.len());
    let mut sum = 0_u32;
    let mut last = 0_u64;
    for (percent, value) in raw_parts {
        sum = if percent == 0 {
            10_000
        } else {
            sum.saturating_add(percent)
        };
        if sum > 10_000 {
            return Err(ConfigError::at(
                directive.location.clone(),
                "percent total is greater than 100%",
            ));
        }
        let threshold = if sum == 10_000 || percent == 0 {
            0
        } else {
            last += percent as u64 * u32::MAX as u64 / 10_000;
            last as u32
        };
        parts.push(SplitClientsPart {
            percent: if sum == 10_000 { 0 } else { percent },
            threshold,
            value,
        });
    }
    Ok(SplitClientsVariable {
        name,
        source,
        parts,
    })
}
