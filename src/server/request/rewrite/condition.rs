use super::*;

pub(in crate::server) fn condition_matches(
    condition: &Condition,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> bool {
    match condition {
        Condition::Truthy(expression) => {
            let value = eval_variables(expression, variables);
            !value.is_empty() && value != "0"
        }
        Condition::Equal {
            left,
            right,
            negate,
        } => {
            let equals = eval_variables(left, variables) == eval_variables(right, variables);
            if *negate { !equals } else { equals }
        }
        Condition::Regex {
            value,
            regex,
            negate,
        } => {
            let value = eval_variables(value, variables);
            let captures = regex.captures(&value);
            let matches = captures.is_some();
            if let Some(captures) = captures.as_ref() {
                set_regex_captures(variables, regex, captures);
            } else {
                clear_regex_captures(variables);
            }
            if variables.rewrite_log_enabled()
                && let Some(logger) = &variables.rewrite_logger
            {
                let result = if matches { "matches" } else { "does not match" };
                logger.log(
                    LogLevel::Notice,
                    format!("\"{}\" {result} \"{value}\"", regex.as_str()),
                );
            }
            if *negate { !matches } else { matches }
        }
        Condition::FileTest {
            operation,
            path,
            negate,
        } => {
            let path_value = eval_variables(path, variables);
            let path = Path::new(&path_value);
            let metadata = check_path_access_sync(path, context, variables)
                .ok()
                .and_then(|()| std::fs::metadata(path).ok());
            let matches = match operation {
                FileTest::Exists => metadata.is_some(),
                FileTest::File => metadata.is_some_and(|metadata| metadata.is_file()),
                FileTest::Directory => metadata.is_some_and(|metadata| metadata.is_dir()),
                FileTest::Executable => executable(&path_value),
            };
            if *negate { !matches } else { matches }
        }
    }
}

#[cfg(unix)]
pub(in crate::server) fn executable(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
pub(in crate::server) fn executable(_path: &str) -> bool {
    false
}
