use super::*;

pub(in crate::server) fn request_body_stream_enabled(
    request: &Request,
    context: &EffectiveConfig,
) -> bool {
    let proxy_streaming = context.proxy_pass.is_some()
        && !context.proxy_request_buffering
        && context.proxy_set_body.is_none()
        && context.proxy_pass_request_body
        && !context.client_body.in_file_only.enabled()
        && !context.proxy_cache_background_update
        && (context.mirrors.is_empty() || !context.mirror_request_body)
        && (context.proxy_http_version != "1.0"
            || request.header("content-length").is_some()
            || request.header("transfer-encoding").is_none());
    request
        .body_stream
        .as_ref()
        .is_some_and(|stream| !stream.is_file_backed())
        && (gateway_request_body_streaming(context, Some(request)) || proxy_streaming)
}

pub(in crate::server) fn request_body_stream_for_upstream<'a>(
    request: &'a Request,
    context: &EffectiveConfig,
) -> Option<&'a crate::http::RequestBodyStream> {
    if request
        .body_stream
        .as_ref()
        .is_some_and(crate::http::RequestBodyStream::is_file_backed)
    {
        return request.body_stream.as_ref();
    }
    request_body_stream_enabled(request, context).then_some(request.body_stream.as_ref()?)
}

pub(in crate::server) fn server_may_stream_request_body(server: &ServerConfig) -> bool {
    config_may_stream_request_body(&server.context)
        || server
            .locations
            .iter()
            .any(location_may_stream_request_body)
}

fn location_may_stream_request_body(location: &LocationConfig) -> bool {
    config_may_stream_request_body(&location.context)
        || location.nested.iter().any(location_may_stream_request_body)
}

fn config_may_stream_request_body(context: &EffectiveConfig) -> bool {
    let proxy_streaming = context.proxy_pass.is_some()
        && !context.proxy_request_buffering
        && context.proxy_set_body.is_none()
        && context.proxy_pass_request_body
        && !context.client_body.in_file_only.enabled()
        && !context.proxy_cache_background_update
        && (context.mirrors.is_empty() || !context.mirror_request_body);
    gateway_request_body_streaming(context, None) || proxy_streaming
}

fn gateway_request_body_streaming(context: &EffectiveConfig, request: Option<&Request>) -> bool {
    let no_competing_consumer = !context.client_body.in_file_only.enabled()
        && (context.mirrors.is_empty() || !context.mirror_request_body);
    if !no_competing_consumer {
        return false;
    }
    let grpc = context.grpc.pass.is_some() && !context.grpc.request_buffering;
    let fastcgi = context.fastcgi_pass.is_some()
        && !context.fastcgi_request_buffering
        && context.fastcgi_pass_request_body
        && !context.fastcgi_cache.background_update;
    let uwsgi = context.uwsgi_pass.is_some()
        && !context.uwsgi_request_buffering
        && context.uwsgi_pass_request_body
        && !context.uwsgi_cache.background_update
        // uWSGI and SCGI have no chunked request framing. Nginx buffers a
        // chunked client body so it can emit a complete body after the headers.
        && request.is_none_or(|request| request.header("content-length").is_some());
    let scgi = context.scgi_pass.is_some()
        && !context.scgi_request_buffering
        && context.scgi_pass_request_body
        && !context.scgi_cache.background_update
        && request.is_none_or(|request| request.header("content-length").is_some());
    grpc || fastcgi || uwsgi || scgi
}

pub(in crate::server) async fn prepare_request_body(
    request: &mut Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> io::Result<()> {
    if variables.request_body_prepared {
        return Ok(());
    }
    variables.request_body_prepared = true;
    validate_declared_request_body_size(request, context.client_max_body_size)?;
    if request_body_stream_enabled(request, context) {
        return Ok(());
    }
    if !should_buffer_request_body(request, context) {
        return Ok(());
    }
    buffer_request_body(request, context).await?;
    validate_buffered_request_body_size(request, context.client_max_body_size)?;
    set_buffered_request_body_variable(request, variables);
    Ok(())
}

pub(super) fn should_buffer_request_body(request: &Request, context: &EffectiveConfig) -> bool {
    request.header("content-length").is_some()
        || request.body_stream.is_none()
        || request_body_buffering_required(request, context)
}

fn request_body_buffering_required(request: &Request, context: &EffectiveConfig) -> bool {
    let proxy = context.proxy_pass.is_some() && context.proxy_pass_request_body;
    let fastcgi = context.fastcgi_pass.is_some() && context.fastcgi_pass_request_body;
    let uwsgi = context.uwsgi_pass.is_some() && context.uwsgi_pass_request_body;
    let scgi = context.scgi_pass.is_some() && context.scgi_pass_request_body;
    let grpc = context.grpc.pass.is_some();
    let mirror = !context.mirrors.is_empty() && context.mirror_request_body;
    let dav_put = request.method == "PUT" && context.dav.allows(crate::config::DavMethod::Put);
    context.client_body.in_file_only.enabled()
        || proxy
        || fastcgi
        || uwsgi
        || scgi
        || grpc
        || mirror
        || dav_put
}

pub(in crate::server) async fn buffer_request_body(
    request: &mut Request,
    context: &EffectiveConfig,
) -> io::Result<()> {
    let body_declared = request.has_request_body();
    if request
        .body_stream
        .as_ref()
        .is_some_and(crate::http::RequestBodyStream::is_file_backed)
    {
        return Ok(());
    }
    if let Some(stream) = request.body_stream.take() {
        stream.set_h2_window_target(i32::MAX as u32);
        let spooled = crate::http::spool_request_body(
            Some(&stream),
            body_declared,
            context.client_body.buffer_size,
            &context.client_body.temp_path,
            context.client_body.in_file_only,
            context.client_max_body_size,
        )
        .await?;
        request.body = spooled.body;
        request.body_stream = spooled.file.map(crate::http::RequestBodyStream::from_file);
        return Ok(());
    }

    let file = store_request_body(
        &request.body,
        body_declared,
        context.client_body.buffer_size,
        &context.client_body.temp_path,
        context.client_body.in_file_only,
    )?;
    if file.is_some() {
        request.body.clear();
    }
    request.body_stream = file.map(crate::http::RequestBodyStream::from_file);
    Ok(())
}

pub(in crate::server) fn set_buffered_request_body_variable(
    request: &Request,
    variables: &mut Variables,
) {
    if variables.request_body_prepared && request.body_stream.is_none() && request.body.is_empty() {
        // Internal redirects intentionally carry no body stream. Keep the
        // prepared variables request-scoped instead of clearing their file.
        return;
    }
    let file = request.body_file();
    variables.request_body = if file.is_some() {
        String::new()
    } else {
        String::from_utf8_lossy(&request.body).into_owned()
    };
    variables.request_body_file = file
        .as_ref()
        .map(|file| file.path().display().to_string())
        .unwrap_or_default();
    variables.request_body_file_guard = file;
}

pub(in crate::server) fn validate_declared_request_body_size(
    request: &Request,
    limit: u64,
) -> io::Result<()> {
    let Some(length) = request.header("content-length") else {
        return Ok(());
    };
    let length = length
        .trim()
        .parse::<u64>()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid Content-Length"))?;
    validate_request_body_size(length, limit)
}

pub(in crate::server) fn validate_buffered_request_body_size(
    request: &Request,
    limit: u64,
) -> io::Result<()> {
    validate_request_body_size(request.buffered_body_len(), limit)
}

fn validate_request_body_size(length: u64, limit: u64) -> io::Result<()> {
    if limit != 0 && length > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body too large",
        ));
    }
    Ok(())
}

pub(in crate::server) fn request_body_error_response(error: &io::Error) -> Response {
    if let Some(status) = crate::http::request_body_error_status(error) {
        return error_response(status);
    }
    if error.kind() == io::ErrorKind::TimedOut {
        error_response(408)
    } else if error.kind() == io::ErrorKind::InvalidData && error.to_string().contains("too large")
    {
        error_response(413)
    } else if error.kind() == io::ErrorKind::InvalidData {
        error_response(400)
    } else {
        error_response(500)
    }
}

pub(in crate::server) fn clear_request_body_variables(variables: &mut Variables) {
    variables.request_body.clear();
    variables.request_body_file.clear();
    variables.request_body_file_guard = None;
    variables.request_body_prepared = true;
}
