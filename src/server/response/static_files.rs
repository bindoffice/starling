use super::*;

pub(in crate::server) fn try_fast_static_file(
    path: &Path,
    context: &EffectiveConfig,
) -> Result<Option<Response>, u16> {
    if context.disable_symlinks.mode != DisableSymlinksMode::Off
        || context.gzip_static != GzipStatic::Off
    {
        return Ok(None);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A special file must not stall an async worker before the type check.
        options.custom_flags(libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(path) else {
        return Ok(None);
    };
    let Ok(metadata) = file.metadata() else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(None);
    }
    let response =
        crate::http::file_response_with_metadata(file, &metadata, content_type(path, context))
            .map_err(|_| 403_u16)?;
    Ok(finalize_static_response(response, &metadata, context))
}

pub(in crate::server) async fn gzip_static_candidate(
    request: &Request,
    path: &Path,
    context: &EffectiveConfig,
    variables: &Variables,
) -> GzipStaticCandidate {
    if context.gzip_static == GzipStatic::Off {
        return GzipStaticCandidate::Off;
    }
    let accepts_gzip = request.header("accept-encoding").is_some_and(accepts_gzip);
    if context.gzip_static == GzipStatic::On && !accepts_gzip && !context.gzip_vary {
        return GzipStaticCandidate::Off;
    }
    let mut compressed_path = path.as_os_str().to_os_string();
    compressed_path.push(".gz");
    let compressed_path = PathBuf::from(compressed_path);
    if let Err(status) = check_path_access(&compressed_path, context, variables).await {
        return if status == 404 {
            GzipStaticCandidate::Off
        } else {
            GzipStaticCandidate::Denied(status)
        };
    }
    let Ok(metadata) = tokio::fs::metadata(&compressed_path).await else {
        return GzipStaticCandidate::Off;
    };
    if !metadata.is_file() {
        return GzipStaticCandidate::Off;
    }
    if context.gzip_static == GzipStatic::On && !accepts_gzip {
        return GzipStaticCandidate::Declined { vary: true };
    }
    let Ok(mut response) =
        crate::http::file_response(&compressed_path, content_type(path, context))
    else {
        return GzipStaticCandidate::Off;
    };
    response.set_header("Content-Encoding", "gzip");
    if context.gzip_static == GzipStatic::On {
        add_vary_accept_encoding(&mut response);
    }
    match finalize_static_response(response, &metadata, context) {
        Some(response) => GzipStaticCandidate::Served(Box::new(response)),
        None => GzipStaticCandidate::Off,
    }
}

pub(in crate::server) fn add_vary_accept_encoding(response: &mut Response) {
    const TOKEN: &str = "Accept-Encoding";
    let Some(vary) = response
        .headers
        .iter_mut()
        .find(|header| header.name.eq_ignore_ascii_case("vary"))
    else {
        response.set_header("Vary", TOKEN);
        return;
    };
    if vary
        .value
        .split(',')
        .any(|value| value.trim().eq_ignore_ascii_case(TOKEN))
    {
        return;
    }
    if !vary.value.trim().is_empty() {
        vary.value.push_str(", ");
    }
    vary.value.push_str(TOKEN);
}

pub(in crate::server) fn finalize_static_response(
    mut response: Response,
    metadata: &std::fs::Metadata,
    context: &EffectiveConfig,
) -> Option<Response> {
    if context.etag {
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let etag = format!("\"{:x}-{:x}\"", modified, metadata.len());
        response.set_header("ETag", &etag);
    }
    Some(response)
}

pub(in crate::server) fn map_request_path(
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    uri: &str,
    add_uri_to_alias: bool,
    variables: &Variables,
) -> PathBuf {
    if let Some(alias) = &context.alias {
        let alias = PathBuf::from(eval_variables(alias.to_string_lossy().as_ref(), variables));
        if matches!(context.alias_uri_prefix.as_deref(), Some("")) {
            return if add_uri_to_alias {
                alias.join(uri.trim_start_matches('/'))
            } else {
                alias
            };
        }
        let suffix = match context.alias_uri_prefix.as_deref() {
            Some(prefix) => uri.strip_prefix(prefix).unwrap_or_default(),
            None => match location.map(|location| &location.matcher) {
                Some(LocationMatcher::Prefix(path))
                | Some(LocationMatcher::PrefixNoRegex(path)) => {
                    uri.strip_prefix(path).unwrap_or_default()
                }
                Some(LocationMatcher::Regex(_)) => uri.trim_start_matches('/'),
                _ => "",
            },
        };
        if suffix.trim_start_matches('/').is_empty() {
            return alias;
        }
        return alias.join(suffix.trim_start_matches('/'));
    }
    context.root.join(uri.trim_start_matches('/'))
}

pub(in crate::server) fn document_root_path(
    context: &EffectiveConfig,
    variables: &Variables,
) -> PathBuf {
    context
        .alias
        .as_ref()
        .map(|alias| PathBuf::from(eval_variables(alias.to_string_lossy().as_ref(), variables)))
        .unwrap_or_else(|| context.root.clone())
}

pub(super) fn log_static_not_found(path: &Path, context: &EffectiveConfig, variables: &Variables) {
    if !context.log_not_found {
        return;
    }
    let Some(logger) = variables.error_logger_for(context) else {
        return;
    };
    logger.log(
        LogLevel::Error,
        format!(
            "open() \"{}\" failed (2: No such file or directory)",
            path.display()
        ),
    );
}

pub(in crate::server) fn content_type(path: &Path, context: &EffectiveConfig) -> String {
    path.extension()
        .and_then(|extension| extension.to_str())
        .and_then(|extension| {
            if let Some(mime) = context.mime_types.get(extension) {
                return Some(mime.clone());
            }
            if extension.bytes().any(|byte| byte.is_ascii_uppercase()) {
                let lowercase = extension.to_ascii_lowercase();
                if let Some(mime) = context.mime_types.get(&lowercase) {
                    return Some(mime.clone());
                }
            }
            HTTP_MIME_TYPES
                .iter()
                .find(|(existing, _)| existing.eq_ignore_ascii_case(extension))
                .map(|(_, mime)| (*mime).to_string())
        })
        .unwrap_or_else(|| context.default_type.clone())
}

pub(in crate::server) const HTTP_MIME_TYPES: &[(&str, &str)] = &[
    ("html", "text/html"),
    ("htm", "text/html"),
    ("shtml", "text/html"),
    ("css", "text/css"),
    ("xml", "text/xml"),
    ("gif", "image/gif"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("js", "application/javascript"),
    ("json", "application/json"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("ico", "image/x-icon"),
    ("txt", "text/plain"),
    ("pdf", "application/pdf"),
    ("wasm", "application/wasm"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("flv", "video/x-flv"),
    ("mp4", "video/mp4"),
];

pub(in crate::server) enum TryFilesResult {
    Found { uri: String, is_dir: bool },
    FallbackStatus(u16),
    FallbackUri(String),
    Named(String),
}
