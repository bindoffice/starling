use super::*;

pub(super) async fn handle(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
) -> Response {
    if path::request_has_body(request) {
        return error::status(415);
    }
    if context.dav.min_delete_depth > 0
        && !meets_min_delete_depth(&env.uri, context.dav.min_delete_depth)
    {
        return error::status(409);
    }

    let path = path::source_path(context, location, &env.uri);
    let metadata = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) => {
            let status = if error.raw_os_error() == Some(libc::ENOTDIR) {
                409
            } else {
                404
            };
            return error::io_error(&error, status);
        }
    };
    if metadata.is_dir() {
        if !env.uri.ends_with('/') {
            return error::status(409);
        }
        if path::depth(request, path::Depth::Infinity).ok() != Some(path::Depth::Infinity) {
            return error::status(400);
        }
    } else {
        let depth = match path::depth(request, path::Depth::Zero) {
            Ok(depth) => depth,
            Err(status) => return error::status(status),
        };
        if matches!(depth, path::Depth::One) {
            return error::status(400);
        }
    }

    match fs_ops::remove(&path, &metadata).await {
        Ok(()) => Response::empty(204),
        Err(error) => error::io_error(&error, 404),
    }
}

fn meets_min_delete_depth(uri: &str, minimum: usize) -> bool {
    let mut depth = 0;
    for (index, byte) in uri.bytes().enumerate() {
        if byte == b'/' {
            depth += 1;
            if depth >= minimum && index + 1 < uri.len() {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_delete_depth_matches_nginx_uri_counting() {
        assert!(!meets_min_delete_depth("/a", 2));
        assert!(meets_min_delete_depth("/a/b", 2));
        assert!(!meets_min_delete_depth("/a/b/", 3));
    }
}
