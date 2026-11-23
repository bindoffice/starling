use super::*;

pub(in crate::server) async fn store_proxy_response(
    response: &UpstreamResponse,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> io::Result<Option<PathBuf>> {
    store_response_body(
        response.status,
        &response.headers,
        &response.body,
        env,
        location,
        context,
        &context.proxy_store,
        context.proxy_store_access,
        variables,
    )
    .await
}

pub(in crate::server) async fn store_gateway_response(
    response: &Response,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> io::Result<Option<PathBuf>> {
    let Some((store, access)) = gateway_store_config(context) else {
        return Ok(None);
    };
    store_response_body(
        response.status,
        &response.headers,
        &response.body,
        env,
        location,
        context,
        store,
        access,
        variables,
    )
    .await
}

pub(in crate::server) fn store_proxy_response_streaming(
    response: &mut UpstreamResponse,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    let _ = attach_stream_response(
        &mut response.body,
        response.status,
        &response.headers,
        env,
        location,
        context,
        &context.proxy_store,
        context.proxy_store_access,
        variables,
    );
}

pub(in crate::server) async fn serve_gateway_store(
    response: &mut Response,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    if response.body.is_stream() {
        let Some((store, access)) = gateway_store_config(context) else {
            return;
        };
        let _ = attach_stream_response(
            &mut response.body,
            response.status,
            &response.headers,
            env,
            location,
            context,
            store,
            access,
            variables,
        );
        return;
    }
    let _ = store_gateway_response(response, env, location, context, variables).await;
}

#[allow(clippy::too_many_arguments)]
async fn store_response_body(
    status: u16,
    headers: &[Header],
    body: &ResponseBody,
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    store: &StoreConfig,
    access: u32,
    variables: &Variables,
) -> io::Result<Option<PathBuf>> {
    if matches!(store, StoreConfig::Off)
        || status != 200
        || !response_body_is_complete(headers, body)
    {
        return Ok(None);
    }

    let mut variables = variables.clone();
    variables.upstream_status = status.to_string();
    variables.upstream_headers = headers.to_vec();
    let Some(path) = store_path(env, location, context, store, &variables) else {
        return Ok(None);
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    tokio::fs::create_dir_all(parent).await?;
    let _ = remove_stale_temporary_files_in_directory(parent, "store").await;

    let temporary = temporary_path(parent, "store");
    let result = async {
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .await?;
        copy_response_body(body, &mut output).await?;
        output.flush().await?;
        drop(output);
        set_metadata(&temporary, modified_time(headers), access).await?;
        replace_file(&temporary, &path).await?;
        Ok(Some(path))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temporary).await;
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn attach_stream_response(
    body: &mut ResponseBody,
    status: u16,
    headers: &[Header],
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    store: &StoreConfig,
    access: u32,
    variables: &Variables,
) -> io::Result<bool> {
    if matches!(store, StoreConfig::Off) || status != 200 {
        return Ok(false);
    }
    let expected_length = content_length(headers).or_else(|| body.content_length());
    let mut variables = variables.clone();
    variables.upstream_status = status.to_string();
    variables.upstream_headers = headers.to_vec();
    let Some(path) = store_path(env, location, context, store, &variables) else {
        return Ok(false);
    };
    store_stream::attach(
        body,
        StoreTarget {
            path,
            access,
            modified: modified_time(headers),
            expected_length,
        },
    )
}

fn store_path(
    env: &RequestEnv,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    store: &StoreConfig,
    variables: &Variables,
) -> Option<PathBuf> {
    match store {
        StoreConfig::Off => None,
        StoreConfig::On => Some(map_request_path(
            context,
            location,
            &env.uri,
            env.add_uri_to_alias,
            variables,
        )),
        StoreConfig::Path(template) => {
            let value = eval_variables(template, variables);
            if value.is_empty() {
                return None;
            }
            let path = PathBuf::from(value);
            Some(if path.is_absolute() {
                path
            } else {
                context.prefix.join(path)
            })
        }
    }
}

fn response_body_is_complete(headers: &[Header], body: &ResponseBody) -> bool {
    content_length(headers).is_none_or(|length| length == body.len())
}

fn content_length(headers: &[Header]) -> Option<u64> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .and_then(|header| header.value.trim().parse::<u64>().ok())
}

fn gateway_store_config(context: &EffectiveConfig) -> Option<(&StoreConfig, u32)> {
    if context.fastcgi_pass.is_some() {
        Some((&context.fastcgi_store, context.fastcgi_store_access))
    } else if context.uwsgi_pass.is_some() {
        Some((&context.uwsgi_store, context.uwsgi_store_access))
    } else if context.scgi_pass.is_some() {
        Some((&context.scgi_store, context.scgi_store_access))
    } else {
        None
    }
}
