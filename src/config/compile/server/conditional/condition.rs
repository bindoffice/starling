use super::*;

pub(super) fn require_location_scope(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    if matches!(scope, Scope::Location) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is not allowed here", directive.name),
    ))
}

pub(super) fn parse_condition(directive: &Directive) -> ConfigResult<Condition> {
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid condition",
        ));
    }
    if !directive.args[0].starts_with('(')
        || !directive
            .args
            .last()
            .is_some_and(|value| value.ends_with(')'))
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid condition",
        ));
    }
    let mut tokens = directive.args.clone();
    let mut leading_parenthesis = false;
    let mut trailing_parenthesis = false;
    if let Some(first) = tokens.first_mut()
        && let Some(rest) = first.strip_prefix('(')
    {
        *first = rest.to_string();
        leading_parenthesis = true;
    }
    if let Some(last) = tokens.last_mut()
        && let Some(rest) = last.strip_suffix(')')
    {
        *last = rest.to_string();
        trailing_parenthesis = true;
    }
    if leading_parenthesis && tokens.first().is_some_and(String::is_empty) {
        tokens.remove(0);
    }
    if trailing_parenthesis && tokens.last().is_some_and(String::is_empty) {
        tokens.pop();
    }

    if tokens.len() == 1 {
        if !is_variable(&tokens[0]) {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid condition",
            ));
        }
        return Ok(Condition::Truthy(tokens.remove(0)));
    }
    if tokens.len() == 2 {
        let (negate, operation) = match tokens[0].as_str() {
            "-f" => (false, FileTest::File),
            "!-f" => (true, FileTest::File),
            "-d" => (false, FileTest::Directory),
            "!-d" => (true, FileTest::Directory),
            "-e" => (false, FileTest::Exists),
            "!-e" => (true, FileTest::Exists),
            "-x" => (false, FileTest::Executable),
            "!-x" => (true, FileTest::Executable),
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid \"if\" condition",
                ));
            }
        };
        return Ok(Condition::FileTest {
            operation,
            path: tokens[1].clone(),
            negate,
        });
    }
    if tokens.len() != 3 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid \"if\" condition",
        ));
    }
    let left = tokens[0].clone();
    if !is_variable(&left) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid condition",
        ));
    }
    let operator = tokens[1].as_str();
    let right = tokens[2].clone();
    match operator {
        "=" => Ok(Condition::Equal {
            left,
            right,
            negate: false,
        }),
        "!=" => Ok(Condition::Equal {
            left,
            right,
            negate: true,
        }),
        "~" | "~*" | "!~" | "!~*" => {
            let regex = RegexBuilder::new(&right)
                .case_insensitive(operator.ends_with('*'))
                .build()
                .map_err(|err| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid regular expression: {err}"),
                    )
                })?;
            Ok(Condition::Regex {
                value: left,
                regex,
                negate: operator.starts_with('!'),
            })
        }
        _ => Err(ConfigError::at(
            directive.location.clone(),
            format!("unexpected \"{operator}\" in condition"),
        )),
    }
}

fn is_variable(value: &str) -> bool {
    value.len() > 1 && value.starts_with('$')
}
