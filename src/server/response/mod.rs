use super::*;

pub(super) async fn serve_static(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    selected_location: Option<&LocationConfig>,
    variables: &Variables,
) -> Option<Response> {
    if !matches!(request.method.as_str(), "GET" | "HEAD" | "POST") {
        let mut response = error_response(405);
        response.set_header("Allow", "GET, HEAD");
        return Some(response);
    }
    if context.empty_gif {
        if request.method == "POST" {
            return Some(error_response(405));
        }
        const EMPTY_GIF: &[u8] = &[
            b'G', b'I', b'F', b'8', b'9', b'a', 0x01, 0x00, 0x01, 0x00, 0x80, 0x01, 0x00, 0x00,
            0x00, 0x00, 0xff, 0xff, 0xff, 0x21, 0xf9, 0x04, 0x01, 0x00, 0x00, 0x01, 0x00, 0x2c,
            0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x4c, 0x01, 0x00,
            0x3b,
        ];
        let mut response = Response::bytes(200, "image/gif", EMPTY_GIF);
        response.set_header(
            "Last-Modified",
            httpdate::fmt_http_date(UNIX_EPOCH + Duration::from_secs(23_349_600)),
        );
        return Some(response);
    }

    if request.method != "POST"
        && let Some(response) = serve_flv(env, context, selected_location, variables).await
    {
        return Some(response);
    }

    let path = map_request_path(
        context,
        selected_location,
        &env.uri,
        env.add_uri_to_alias,
        variables,
    );
    if request.method != "POST" {
        match try_fast_static_file(&path, context) {
            Ok(Some(response)) => return Some(response),
            Ok(None) => {}
            Err(status) => return Some(error_response(status)),
        }
    }
    if request.method != "POST" && !env.uri.ends_with('/') {
        match gzip_static_candidate(request, &path, context, variables).await {
            GzipStaticCandidate::Served(response) => return Some(*response),
            GzipStaticCandidate::Denied(status) => return Some(error_response(status)),
            GzipStaticCandidate::Off | GzipStaticCandidate::Declined { .. } => {}
        }
    }
    if let Err(status) = check_path_access(&path, context, variables).await {
        return Some(error_response(status));
    }
    let metadata = match tokio::fs::metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            if error.kind() == io::ErrorKind::NotFound {
                log_static_not_found(&path, context, variables);
            }
            return None;
        }
    };
    if metadata.is_dir() {
        if !env.uri.ends_with('/') {
            let mut location = format!("{}/", escape_nginx_uri(&env.uri));
            if let Some(args) = &env.args {
                location.push('?');
                location.push_str(args);
            }
            let mut response = Response::empty(301);
            response.set_header("Location", location);
            return Some(response);
        }
        for index in &context.index {
            let candidate = path.join(index);
            match check_path_access(&candidate, context, variables).await {
                Ok(()) => {}
                Err(404) => continue,
                Err(status) => return Some(error_response(status)),
            }
            if let Ok(index_metadata) = tokio::fs::metadata(&candidate).await
                && index_metadata.is_file()
            {
                return static_file_response(
                    request,
                    &candidate,
                    index_metadata,
                    context,
                    variables,
                )
                .await;
            }
        }
        if context.random_index
            && let Some(candidate) = random_index_file(&path).await
            && let Ok(()) = check_path_access(&candidate, context, variables).await
            && let Ok(metadata) = tokio::fs::metadata(&candidate).await
        {
            return static_file_response(request, &candidate, metadata, context, variables).await;
        }
        if context.autoindex {
            if request.method == "POST" {
                return Some(error_response(405));
            }
            return Some(autoindex_response(request, &path, &env.uri, context).await);
        }
        return Some(error_response(403));
    }
    if !metadata.is_file() {
        return Some(error_response(404));
    }
    if request.method == "POST" {
        let mut response = error_response(405);
        response.set_header("Allow", "GET, HEAD");
        return Some(response);
    }
    static_file_response(request, &path, metadata, context, variables).await
}

pub(super) async fn random_index_file(path: &Path) -> Option<PathBuf> {
    static RANDOM_INDEX_COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut files = Vec::new();
    let mut directory = tokio::fs::read_dir(path).await.ok()?;
    while let Ok(Some(entry)) = directory.next_entry().await {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type().await.is_ok_and(|kind| kind.is_file()) {
            files.push(entry.path());
        }
    }
    if files.is_empty() {
        return None;
    }
    files.sort();
    let counter = RANDOM_INDEX_COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let index = (counter.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ now) % files.len() as u64;
    files.into_iter().nth(index as usize)
}

pub(super) async fn static_file_response(
    request: &Request,
    path: &Path,
    metadata: std::fs::Metadata,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<Response> {
    if request.method == "POST" {
        let mut response = error_response(405);
        response.set_header("Allow", "GET, HEAD");
        return Some(response);
    }
    if let Err(status) = check_path_access(path, context, variables).await {
        return Some(error_response(status));
    }
    let gzip_candidate = gzip_static_candidate(request, path, context, variables).await;
    if let GzipStaticCandidate::Served(response) = gzip_candidate {
        return Some(*response);
    }
    if let GzipStaticCandidate::Denied(status) = gzip_candidate {
        return Some(error_response(status));
    }
    let content_type = content_type(path, context);
    let mut response = match crate::http::file_response(path, content_type) {
        Ok(response) => response,
        Err(_) => return Some(error_response(403)),
    };
    if matches!(gzip_candidate, GzipStaticCandidate::Declined { vary: true }) {
        add_vary_accept_encoding(&mut response);
    }
    finalize_static_response(response, &metadata, context)
}

pub(super) enum GzipStaticCandidate {
    Off,
    Declined { vary: bool },
    Served(Box<Response>),
    Denied(u16),
}

mod addition;
mod autoindex;
mod charset;
mod dav;
mod error_pages;
mod etag;
mod expires;
mod filters;
mod flv;
mod gunzip;
mod gzip_rules;
mod headers;
mod msie;
mod not_modified;
mod ranges;
mod sent;
mod ssi;
mod static_files;
mod symlinks;
mod trailers;
mod upstream_ranges;
mod userid;

pub(super) use addition::*;
pub(super) use autoindex::*;
pub(super) use charset::*;
pub(super) use dav::*;
pub(super) use error_pages::*;
pub(super) use etag::*;
pub(super) use expires::*;
pub(super) use filters::*;
pub(super) use flv::*;
pub(super) use gunzip::*;
pub(super) use headers::*;
pub(super) use msie::*;
pub(super) use not_modified::*;
pub(super) use ranges::*;
pub(super) use sent::*;
pub(super) use ssi::*;
pub(super) use static_files::*;
pub(super) use symlinks::*;
pub(super) use trailers::*;
pub(super) use upstream_ranges::*;
pub(super) use userid::*;
