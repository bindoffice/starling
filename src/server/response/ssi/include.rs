use super::eval::{Runtime, render};
use super::parser::{self, SsiTag};
use super::state::State;
use super::text::evaluate_text;
use super::*;

pub(super) async fn include_command(
    tag: &SsiTag,
    runtime: &mut Runtime<'_>,
    state: &mut State,
) -> Result<Vec<u8>, String> {
    validate_include_params(tag)?;
    let virtual_target = param(tag, "virtual");
    let file_target = param(tag, "file");
    if virtual_target.is_some() && file_target.is_some() {
        return Err("inclusion may be either virtual or file".into());
    }
    let Some(target) = virtual_target.or(file_target) else {
        return Err("no parameter in \"include\" SSI command".into());
    };
    if file_target.is_some() && param(tag, "wait").is_some() {
        return Err("\"wait\" cannot be used with file".into());
    }
    if let Some(wait) = param(tag, "wait")
        && !wait.eq_ignore_ascii_case("yes")
        && !wait.eq_ignore_ascii_case("no")
    {
        return Err(format!(
            "invalid value \"{wait}\" in the \"wait\" parameter"
        ));
    }
    if param(tag, "set").is_some() && param(tag, "stub").is_some() {
        return Err("\"set\" and \"stub\" cannot be used together".into());
    }

    let target = evaluate_text(target, state, runtime.variables)?;
    let target = resolve_target(&target, &runtime.variables.uri);

    if let Some(stub) = param(tag, "stub") {
        let _ = process_target(target, runtime).await?;
        let block = state
            .blocks
            .iter()
            .find(|(name, _)| name == stub)
            .map(|(_, body)| body.clone())
            .ok_or_else(|| format!("\"stub\"=\"{stub}\" for \"include\" not found"))?;
        let segments = parser::parse(&block, runtime.context.ssi.value_length);
        return Box::pin(render(&segments, runtime, state)).await;
    }

    let set_name = param(tag, "set").map(variable_name);
    if let Some(name) = set_name {
        state.variables.entry(name.to_string()).or_default();
    }
    let included = process_target(target, runtime).await?;
    if let Some(set) = set_name {
        if included.status < 300 {
            state.variables.insert(
                set.to_string(),
                String::from_utf8_lossy(&included.body).into_owned(),
            );
        }
        return Ok(Vec::new());
    }
    Ok(included.body)
}

struct Included {
    status: u16,
    body: Vec<u8>,
}

async fn process_target(target: String, runtime: &mut Runtime<'_>) -> Result<Included, String> {
    let (path, args) = rewrite_destination(&target, &None);
    let uri = normalize_uri(&path).ok_or_else(|| "invalid URI in SSI include".to_string())?;
    let env = RequestEnv {
        uri: uri.clone(),
        args: args.clone(),
        unparsed_uri: Some(append_args(uri.clone(), args)),
        internal: true,
        bypass_try_files: false,
        limits_checked: true,
        auth_request_checked: true,
        mirror_checked: true,
    };
    let mut subrequest = runtime.request.clone();
    subrequest.method = "GET".into();
    subrequest.target = env.unparsed_uri.clone().unwrap_or_else(|| uri.clone());
    subrequest.body.clear();
    subrequest.body_stream = None;
    subrequest.headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.name.eq_ignore_ascii_case("expect")
    });
    let mut subvariables = runtime.variables.clone();
    subvariables.method = "GET".into();
    subvariables.request_target = subrequest.target.clone();
    clear_request_body_variables(&mut subvariables);
    let processed = Box::pin(process_request(
        runtime.http,
        runtime.server,
        &mut subrequest,
        env,
        &mut subvariables,
        runtime.depth + 1,
    ))
    .await;
    let mut response = processed.response;
    if ssi_applies(&processed.context, &subrequest, &response) {
        Box::pin(apply_ssi(
            runtime.http,
            runtime.server,
            &subrequest,
            &mut response,
            &processed.context,
            &mut subvariables,
            runtime.depth + 1,
        ))
        .await;
    }
    let body = match std::mem::replace(&mut response.body, ResponseBody::Empty)
        .into_memory(MAX_BODY_BYTES as u64)
        .await
    {
        Ok(body) => body,
        Err(body) => {
            response.body = *body;
            Vec::new()
        }
    };
    Ok(Included {
        status: response.status,
        body,
    })
}

fn resolve_target(target: &str, current_uri: &str) -> String {
    if target.starts_with('/') {
        return target.to_string();
    }
    let directory = current_uri
        .rfind('/')
        .map(|index| &current_uri[..=index])
        .unwrap_or("/");
    format!("{directory}{target}")
}

fn validate_include_params(tag: &SsiTag) -> Result<(), String> {
    let allowed = ["virtual", "file", "wait", "set", "stub"];
    for parameter in &tag.params {
        if !allowed.contains(&parameter.name.as_str()) {
            return Err(format!(
                "invalid parameter \"{}\" in \"include\" SSI command",
                parameter.name
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
                "duplicate \"{}\" parameter in \"include\" SSI command",
                parameter.name
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

fn variable_name(value: &str) -> &str {
    value.strip_prefix('$').unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_includes_use_the_current_directory() {
        assert_eq!(
            resolve_target("footer.html", "/docs/index.html"),
            "/docs/footer.html"
        );
        assert_eq!(
            resolve_target("/shared.html", "/docs/index.html"),
            "/shared.html"
        );
        assert_eq!(resolve_target("footer.html", "/"), "/footer.html");
    }
}
