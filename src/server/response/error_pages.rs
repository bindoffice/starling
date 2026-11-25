use super::*;

pub(in crate::server) async fn resolve_try_files(
    try_files: &crate::config::TryFiles,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    env: &RequestEnv,
    variables: &Variables,
) -> TryFilesResult {
    for candidate in &try_files.candidates {
        let test_dir = candidate.ends_with('/') && candidate.len() > 1;
        let candidate = if test_dir {
            &candidate[..candidate.len() - 1]
        } else {
            candidate.as_str()
        };
        let candidate = eval_variables(candidate, variables);
        let uri = if candidate.starts_with('/') {
            candidate.to_string()
        } else {
            let parent = env
                .uri
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or("");
            format!("{parent}/{candidate}")
        };
        let normalized = normalize_uri(&uri).unwrap_or(uri);
        let path = map_request_path(
            context,
            location,
            &normalized,
            matches!(context.alias_uri_prefix.as_deref(), Some("")),
            variables,
        );
        if check_path_access(&path, context, variables).await.is_err() {
            continue;
        }
        if let Ok(metadata) = tokio::fs::metadata(&path).await
            && if test_dir {
                metadata.is_dir()
            } else {
                metadata.is_file()
            }
        {
            return TryFilesResult::Found {
                uri: normalized,
                is_dir: test_dir,
            };
        }
    }
    let fallback = eval_variables(&try_files.fallback, variables);
    if let Some(status) = fallback.strip_prefix('=') {
        return TryFilesResult::FallbackStatus(status.parse::<u16>().unwrap_or(500));
    }
    if fallback.starts_with('@') {
        return TryFilesResult::Named(fallback);
    }
    TryFilesResult::FallbackUri(fallback)
}

pub(in crate::server) fn find_error_page(pages: &[ErrorPage], status: u16) -> Option<&ErrorPage> {
    pages.iter().find(|page| page.statuses.contains(&status))
}

pub(in crate::server) fn intercepts_upstream_status(
    status: u16,
    intercept_errors: bool,
    pages: &[ErrorPage],
) -> bool {
    status >= 300 && intercept_errors && find_error_page(pages, status).is_some()
}

pub(in crate::server) struct ErrorPageOutcome {
    pub(in crate::server) response: Response,
    pub(in crate::server) context: Arc<EffectiveConfig>,
}

pub(in crate::server) async fn configured_error_page(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    response: Response,
    context: &Arc<EffectiveConfig>,
    variables: &mut Variables,
    depth: usize,
) -> ErrorPageOutcome {
    if variables.error_page || context.error_pages.is_empty() {
        return ErrorPageOutcome {
            response,
            context: context.clone(),
        };
    }
    hydrate_error_page_body(request, variables).await;
    let auth_challenges = preserved_auth_challenges(&response);
    let redirect_location = response.header("Location").map(str::to_string);
    // Nginx sets r->error_page before searching and keeps it request-scoped.
    if !context.recursive_error_pages {
        variables.error_page = true;
    }
    let Some(page) = find_error_page(&context.error_pages, response.status) else {
        return ErrorPageOutcome {
            response,
            context: context.clone(),
        };
    };
    let early_hints = response.early_hints().to_vec();
    let Some(mut redirected) = Box::pin(follow_error_page(
        http,
        server,
        request,
        page,
        response.status,
        context,
        variables,
        depth + 1,
    ))
    .await
    else {
        return ErrorPageOutcome {
            response,
            context: context.clone(),
        };
    };
    for challenge in auth_challenges {
        redirected
            .response
            .add_header(&challenge.name, challenge.value);
    }
    if redirected.response.header("Location").is_none()
        && let Some(location) = redirect_location
    {
        redirected.response.set_header("Location", location);
    }
    redirected.response.set_early_hints(early_hints);
    ErrorPageOutcome {
        response: redirected.response,
        context: redirected.context,
    }
}

async fn hydrate_error_page_body(request: &Request, variables: &mut Variables) {
    if !variables.request_body.is_empty() || !variables.request_body_file.is_empty() {
        return;
    }
    let Some(stream) = request.body_stream.as_ref() else {
        return;
    };
    let Some(body) = stream.captured_body().await else {
        return;
    };
    variables.request_body = String::from_utf8_lossy(&body).into_owned();
}

fn preserved_auth_challenges(response: &Response) -> Vec<Header> {
    if response.status != 401 {
        return Vec::new();
    }
    let headers = if response.upstream_control().is_some() {
        response.upstream_headers()
    } else {
        response.headers.as_slice()
    };
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("WWW-Authenticate"))
        .cloned()
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn follow_error_page(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    page: &ErrorPage,
    original_status: u16,
    current_context: &Arc<EffectiveConfig>,
    variables: &mut Variables,
    depth: usize,
) -> Option<Processed> {
    let uri = eval_variables(&page.uri, variables);
    if uri.starts_with('@') {
        let mut internal_request = internal_redirect_request(request);
        if !internal_request.method.eq_ignore_ascii_case("HEAD") {
            variables.method = "GET".into();
        }
        let env = RequestEnv {
            uri: variables.uri.clone(),
            args: None,
            unparsed_uri: None,
            valid_location: true,
            internal: true,
            bypass_try_files: true,
            add_uri_to_alias: false,
            limits_checked: true,
            auth_request_checked: true,
            mirror_checked: true,
        };
        let mut connection_guard = None;
        let mut processed = process_named_location(
            http,
            server,
            &mut internal_request,
            &uri,
            env,
            variables,
            depth,
            MAX_URI_CHANGES + 1,
            &mut connection_guard,
        )
        .await?;
        apply_error_page_overwrite(&mut processed.response, page.overwrite, original_status);
        return Some(processed);
    }
    if uri.starts_with("http://") || uri.starts_with("https://") {
        return Some(Processed {
            response: error_page_redirect(&uri, page.overwrite),
            context: current_context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    if !uri.starts_with('/') {
        return Some(Processed {
            response: error_page_redirect(&uri, page.overwrite),
            context: current_context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    let (uri, args) = match uri.split_once('?') {
        Some((uri, args)) => (uri, Some(args.to_string())),
        None => (uri.as_str(), None),
    };
    let uri = normalize_uri(uri)?;
    let env = RequestEnv {
        uri,
        args,
        unparsed_uri: None,
        valid_location: true,
        internal: true,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: true,
        auth_request_checked: true,
        mirror_checked: true,
    };
    let mut internal_request = internal_redirect_request(request);
    if !internal_request.method.eq_ignore_ascii_case("HEAD") {
        variables.method = "GET".into();
    }
    let mut processed = Box::pin(process_request(
        http,
        server,
        &mut internal_request,
        env,
        variables,
        depth + 1,
    ))
    .await;
    apply_error_page_overwrite(&mut processed.response, page.overwrite, original_status);
    Some(processed)
}

pub(in crate::server) fn internal_redirect_request(request: &Request) -> Request {
    let mut internal = request.clone();
    if !internal.method.eq_ignore_ascii_case("HEAD") {
        internal.method = "GET".into();
    }
    // Nginx keeps an already buffered request body available to the redirected
    // location. Live client-body streams still have to be discarded: the
    // client connection may already be parsing the next request.
    if internal
        .body_stream
        .as_ref()
        .is_some_and(|stream| !stream.is_file_backed())
    {
        internal.body.clear();
        internal.body_stream = None;
    }
    internal.headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.name.eq_ignore_ascii_case("expect")
    });
    internal
}

pub(in crate::server) fn apply_error_page_overwrite(
    response: &mut Response,
    overwrite: ErrorPageOverwrite,
    original_status: u16,
) {
    response.status = match overwrite {
        ErrorPageOverwrite::Preserve if response.status < 300 => original_status,
        ErrorPageOverwrite::Preserve => response.status,
        ErrorPageOverwrite::Response => response.status,
        ErrorPageOverwrite::Status(status) => status,
    };
}

pub(in crate::server) fn error_page_redirect(
    location: &str,
    overwrite: ErrorPageOverwrite,
) -> Response {
    let status = match overwrite {
        ErrorPageOverwrite::Status(status) if (300..400).contains(&status) => status,
        _ => 302,
    };
    let mut response = Response::empty(status);
    response.set_header("Location", location);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_all_upstream_www_authenticate_values_for_401_error_pages() {
        let mut response = Response::new(401);
        response.add_header("WWW-Authenticate", "filtered-value");
        response.upstream_control_mut().headers = vec![
            Header {
                name: "WWW-Authenticate".into(),
                value: "Basic realm=\"one\"".into(),
            },
            Header {
                name: "Content-Type".into(),
                value: "text/plain".into(),
            },
            Header {
                name: "www-authenticate".into(),
                value: "Bearer realm=\"two\"".into(),
            },
        ];

        let challenges = preserved_auth_challenges(&response);
        assert_eq!(challenges.len(), 2);
        assert_eq!(challenges[0].value, "Basic realm=\"one\"");
        assert_eq!(challenges[1].value, "Bearer realm=\"two\"");
    }

    #[test]
    fn does_not_preserve_auth_challenges_for_non_401_responses() {
        let mut response = Response::new(403);
        response.add_header("WWW-Authenticate", "Basic realm=\"one\"");
        assert!(preserved_auth_challenges(&response).is_empty());
    }

    #[test]
    fn preserves_locally_generated_auth_challenges_when_there_is_no_upstream_response() {
        let mut response = Response::new(401);
        response.add_header("WWW-Authenticate", "Basic realm=\"local\"");
        assert_eq!(preserved_auth_challenges(&response).len(), 1);
    }

    #[test]
    fn does_not_mix_local_headers_into_upstream_auth_challenges() {
        let mut response = Response::new(401);
        response.add_header("WWW-Authenticate", "Basic realm=\"local\"");
        response.upstream_control_mut().headers.clear();
        assert!(preserved_auth_challenges(&response).is_empty());
    }

    #[test]
    fn error_page_lookup_uses_the_first_matching_declaration() {
        let pages = vec![
            ErrorPage {
                statuses: vec![404],
                overwrite: ErrorPageOverwrite::Preserve,
                uri: "/first".into(),
            },
            ErrorPage {
                statuses: vec![404],
                overwrite: ErrorPageOverwrite::Preserve,
                uri: "/second".into(),
            },
        ];

        assert_eq!(
            find_error_page(&pages, 404).map(|page| page.uri.as_str()),
            Some("/first")
        );
    }

    #[test]
    fn interception_requires_an_enabled_exact_match_at_or_above_300() {
        let pages = vec![
            ErrorPage {
                statuses: vec![302],
                overwrite: ErrorPageOverwrite::Preserve,
                uri: "/redirect-error".into(),
            },
            ErrorPage {
                statuses: vec![404],
                overwrite: ErrorPageOverwrite::Preserve,
                uri: "/not-found".into(),
            },
        ];

        assert!(intercepts_upstream_status(302, true, &pages));
        assert!(intercepts_upstream_status(404, true, &pages));
        assert!(!intercepts_upstream_status(404, false, &pages));
        assert!(!intercepts_upstream_status(500, true, &pages));
        assert!(!intercepts_upstream_status(299, true, &pages));
    }
}
