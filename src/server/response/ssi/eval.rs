use super::include::include_command;
use super::parser::{Segment, SsiTag};
use super::state::{Conditional, Encoding, State};
use super::text::{entity_encode, evaluate_text, lookup_variable, url_encode};
use super::*;

pub(super) struct Runtime<'a> {
    pub(super) http: &'a HttpConfig,
    pub(super) server: &'a ServerConfig,
    pub(super) request: &'a Request,
    pub(super) context: &'a EffectiveConfig,
    pub(super) variables: &'a mut Variables,
    pub(super) depth: usize,
}

pub(super) async fn render(
    segments: &[Segment],
    runtime: &mut Runtime<'_>,
    state: &mut State,
) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    for segment in segments {
        match segment {
            Segment::Text(text) => {
                if state.block.is_some() {
                    state.block_buffer.extend_from_slice(text);
                } else if state.output {
                    output.extend_from_slice(text);
                }
            }
            Segment::Directive { raw, tag } => {
                if state.block.is_some() {
                    if tag
                        .as_ref()
                        .is_some_and(|tag| tag.command.eq_ignore_ascii_case("endblock"))
                    {
                        finish_block(state);
                    } else {
                        state.block_buffer.extend_from_slice(raw);
                    }
                    continue;
                }

                let Some(tag) = tag else {
                    if state.output && !runtime.context.ssi.silent_errors {
                        output.extend_from_slice(state.errmsg.as_bytes());
                    }
                    continue;
                };
                if !state.output && !is_control_command(&tag.command) {
                    continue;
                }

                match execute(tag, runtime, state).await {
                    Ok(Some(value)) => {
                        if state.output {
                            output.extend_from_slice(&value);
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !runtime.context.ssi.silent_errors {
                            output.extend_from_slice(state.errmsg.as_bytes());
                        }
                        let _ = error;
                    }
                }
            }
        }
    }
    if let Some(name) = state.block.take() {
        let block = std::mem::take(&mut state.block_buffer);
        state.blocks.push((name, block));
    }
    Ok(output)
}

fn finish_block(state: &mut State) {
    if let Some(name) = state.block.take() {
        let block = std::mem::take(&mut state.block_buffer);
        state.blocks.push((name, block));
    }
    state.output = true;
}

fn is_control_command(command: &str) -> bool {
    matches!(
        command.to_ascii_lowercase().as_str(),
        "if" | "elif" | "else" | "endif" | "endblock"
    )
}

async fn execute(
    tag: &SsiTag,
    runtime: &mut Runtime<'_>,
    state: &mut State,
) -> Result<Option<Vec<u8>>, String> {
    match tag.command.to_ascii_lowercase().as_str() {
        "include" => include_command(tag, runtime, state).await.map(Some),
        "echo" => echo(tag, runtime, state).map(Some),
        "config" => config(tag, state),
        "set" => set(tag, runtime, state),
        "if" | "elif" => if_command(tag, runtime, state),
        "else" => else_command(tag, state),
        "endif" => endif_command(tag, state),
        "block" => block_command(tag, state),
        "endblock" => {
            finish_block(state);
            Ok(None)
        }
        command => Err(format!("unknown SSI command \"{command}\"")),
    }
}

fn config(tag: &SsiTag, state: &mut State) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &["errmsg", "timefmt"])?;
    if let Some(value) = param(tag, "timefmt") {
        state.timefmt = value.to_string();
    }
    if let Some(value) = param(tag, "errmsg") {
        state.errmsg = value.to_string();
    }
    Ok(None)
}

fn set(tag: &SsiTag, runtime: &Runtime<'_>, state: &mut State) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &["var", "value"])?;
    let name = required_param(tag, "var")?;
    let value = required_param(tag, "value")?;
    let value = evaluate_text(value, state, runtime.variables)?;
    state
        .variables
        .insert(variable_name(name).to_string(), value);
    Ok(None)
}

fn echo(tag: &SsiTag, runtime: &Runtime<'_>, state: &mut State) -> Result<Vec<u8>, String> {
    validate_params(tag, &["var", "default", "encoding"])?;
    let name = required_param(tag, "var")?;
    let name = variable_name(name);
    if let Some(encoding) = param(tag, "encoding") {
        state.encoding = match encoding {
            "none" => Encoding::None,
            "url" => Encoding::Url,
            "entity" => Encoding::Entity,
            _ => state.encoding,
        };
    }
    if state.variables.get(name).is_some_and(String::is_empty) {
        return Ok(Vec::new());
    }
    let value = match lookup_variable(name, state, runtime.variables) {
        Some(value) => value,
        None => param(tag, "default").map_or_else(|| "(none)".into(), str::to_string),
    };
    if value.is_empty() {
        return Ok(Vec::new());
    }
    Ok(match state.encoding {
        Encoding::None => value.into_bytes(),
        Encoding::Url => url_encode(&value).into_bytes(),
        Encoding::Entity => entity_encode(&value).into_bytes(),
    })
}

fn if_command(
    tag: &SsiTag,
    runtime: &mut Runtime<'_>,
    state: &mut State,
) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &["expr"])?;
    let is_elif = tag.command.eq_ignore_ascii_case("elif");
    if is_elif {
        if state.conditional != Conditional::If {
            return Err("invalid context of SSI command \"elif\"".into());
        }
        if state.output_chosen {
            state.output = false;
            return Ok(None);
        }
    } else if state.conditional != Conditional::None {
        return Err("the \"if\" command inside the \"if\" command".into());
    }

    let expression = required_param(tag, "expr")?;
    let matched = evaluate_condition(expression, runtime, state)?;
    state.conditional = Conditional::If;
    state.output = matched;
    if matched {
        state.output_chosen = true;
    }
    Ok(None)
}

fn else_command(tag: &SsiTag, state: &mut State) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &[])?;
    if state.conditional != Conditional::If {
        return Err("invalid context of SSI command \"else\"".into());
    }
    state.output = !state.output_chosen;
    state.conditional = Conditional::Else;
    Ok(None)
}

fn endif_command(tag: &SsiTag, state: &mut State) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &[])?;
    if state.conditional == Conditional::None {
        return Err("invalid context of SSI command \"endif\"".into());
    }
    state.output = true;
    state.output_chosen = false;
    state.conditional = Conditional::None;
    Ok(None)
}

fn block_command(tag: &SsiTag, state: &mut State) -> Result<Option<Vec<u8>>, String> {
    validate_params(tag, &["name"])?;
    let name = required_param(tag, "name")?.to_string();
    state.block = Some(name);
    state.block_buffer.clear();
    state.output = false;
    Ok(None)
}

fn evaluate_condition(
    expression: &str,
    runtime: &mut Runtime<'_>,
    state: &mut State,
) -> Result<bool, String> {
    let (left, operator, right) = split_condition(expression)?;
    let left = evaluate_text(left, state, runtime.variables)?;
    let Some(operator) = operator else {
        return Ok(!left.is_empty());
    };
    if right.len() > 2 && right.starts_with('/') {
        if !right.ends_with('/') {
            return Err("invalid SSI expression".into());
        }
        let pattern = evaluate_text(&right[1..right.len() - 1], state, runtime.variables)?;
        let regex = crate::config::Regex::new(&pattern)
            .map_err(|error| format!("invalid SSI regular expression: {error}"))?;
        if let Some(captures) = regex.captures(&left) {
            state.captures = captures
                .iter()
                .map(|capture| capture.map_or_else(String::new, |capture| capture.as_str().into()))
                .collect();
            let names = regex.capture_names().collect::<Vec<_>>();
            state.named_captures.clear();
            for (index, name) in names.into_iter().enumerate() {
                if let Some(name) = name
                    && let Some(capture) = captures.get(index)
                {
                    state
                        .named_captures
                        .insert(name.to_string(), capture.as_str().to_string());
                }
            }
            return Ok(operator == "=");
        }
        return Ok(operator == "!=");
    }
    let right = evaluate_text(right, state, runtime.variables)?;
    let equal = left == right;
    Ok(if operator == "=" { equal } else { !equal })
}

fn split_condition(expression: &str) -> Result<(&str, Option<&str>, &str), String> {
    let mut quote = None;
    let mut escaped = false;
    let bytes = expression.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            index += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            index += 1;
            continue;
        }
        if expression[index..].starts_with("!=") {
            return Ok((
                expression[..index].trim(),
                Some("!="),
                expression[index + 2..].trim(),
            ));
        }
        if byte == b'=' {
            return Ok((
                expression[..index].trim(),
                Some("="),
                expression[index + 1..].trim(),
            ));
        }
        index += 1;
    }
    Ok((expression.trim(), None, ""))
}

fn variable_name(value: &str) -> &str {
    value.strip_prefix('$').unwrap_or(value)
}

fn validate_params(tag: &SsiTag, allowed: &[&str]) -> Result<(), String> {
    for parameter in &tag.params {
        if !allowed.contains(&parameter.name.as_str()) {
            return Err(format!(
                "invalid parameter \"{}\" in \"{}\" SSI command",
                parameter.name, tag.command
            ));
        }
        if tag
            .params
            .iter()
            .filter(|candidate| candidate.name == parameter.name)
            .count()
            > 1
        {
            return Err(format!(
                "duplicate \"{}\" parameter in \"{}\" SSI command",
                parameter.name, tag.command
            ));
        }
    }
    Ok(())
}

fn param<'a>(tag: &'a SsiTag, name: &str) -> Option<&'a str> {
    tag.params
        .iter()
        .find(|parameter| parameter.name == name)
        .map(|parameter| parameter.value.as_str())
}

fn required_param<'a>(tag: &'a SsiTag, name: &str) -> Result<&'a str, String> {
    param(tag, name).ok_or_else(|| {
        format!(
            "mandatory \"{name}\" parameter is absent in \"{}\" SSI command",
            tag.command
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_splitting_respects_quotes() {
        assert_eq!(
            split_condition(r#"$name = "a=b""#).unwrap(),
            ("$name", Some("="), r#""a=b""#)
        );
        assert_eq!(
            split_condition(r#"$name != "value""#).unwrap(),
            ("$name", Some("!="), r#""value""#)
        );
    }

    #[test]
    fn entity_encoding_matches_ssi_defaults() {
        assert_eq!(entity_encode(r#"a&<b>"#), "a&amp;&lt;b&gt;");
    }
}
