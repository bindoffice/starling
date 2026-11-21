use super::*;

pub(in crate::server) fn condition_matches(
    condition: &Condition,
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
            if variables.rewrite_log_enabled()
                && let Some(logger) = &variables.rewrite_logger
            {
                let result = if matches { "matches" } else { "does not match" };
                logger.log(
                    LogLevel::Notice,
                    format!("\"{}\" {result} \"{value}\"", regex.as_str()),
                );
            }
            if matches && !*negate {
                set_regex_captures(variables, regex, captures.as_ref().unwrap());
            }
            if *negate { !matches } else { matches }
        }
        Condition::FileTest {
            operation,
            path,
            negate,
        } => {
            let path = eval_variables(path, variables);
            let metadata = std::fs::metadata(&path);
            let matches = match operation {
                FileTest::Exists => metadata.is_ok(),
                FileTest::File => metadata.is_ok_and(|metadata| metadata.is_file()),
                FileTest::Directory => metadata.is_ok_and(|metadata| metadata.is_dir()),
                FileTest::Executable => executable(&path),
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
