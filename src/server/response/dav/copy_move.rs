use super::*;

pub(super) async fn handle(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    method: DavMethod,
) -> Response {
    if path::request_has_body(request) {
        return error::status(415);
    }
    let destination = match path::parse_destination(request, context, location) {
        Ok(destination) => destination,
        Err(status) => return error::status(status),
    };
    if env.uri.ends_with('/') != destination.uri.ends_with('/') {
        return error::status(409);
    }
    let depth = match path::depth(request, path::Depth::Infinity) {
        Ok(depth) => depth,
        Err(status) => return error::status(status),
    };
    if (method == DavMethod::Copy && !matches!(depth, path::Depth::Zero | path::Depth::Infinity))
        || (method == DavMethod::Move && depth != path::Depth::Infinity)
    {
        return error::status(400);
    }
    let overwrite = match request.header("overwrite") {
        None => true,
        Some(value) if value.eq_ignore_ascii_case("T") => true,
        Some(value) if value.eq_ignore_ascii_case("F") => false,
        Some(_) => return error::status(400),
    };

    let source = path::source_path(context, location, &env.uri);
    if path::same_or_descendant(&source, &destination.path) {
        return error::status(403);
    }
    let source_metadata = match tokio::fs::metadata(&source).await {
        Ok(metadata) => metadata,
        Err(error) => return error::io_error(&error, 404),
    };
    if source_metadata.is_dir() && !env.uri.ends_with('/') {
        return error::status(400);
    }
    if source_metadata.is_dir()
        && destination.path.starts_with(&source)
        && destination.path != source
    {
        return error::status(403);
    }

    let destination_metadata = tokio::fs::symlink_metadata(&destination.path).await.ok();
    if destination_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.is_dir() && !destination.uri.ends_with('/'))
    {
        return error::status(409);
    }
    if destination_metadata.is_some() && !overwrite {
        return error::status(412);
    }
    if let Some(metadata) = &destination_metadata
        && let Err(error) = fs_ops::remove(&destination.path, metadata).await
    {
        return error::io_error(&error, 404);
    }

    if method == DavMethod::Move
        && let Err(error) = fs_ops::create_parent_dirs(&destination.path, context.dav.access).await
    {
        return error::io_error(&error, 500);
    }

    if source_metadata.is_dir() {
        if method == DavMethod::Move && tokio::fs::rename(&source, &destination.path).await.is_ok()
        {
            return Response::empty(201);
        }
        if let Err(error) = fs_ops::copy(&source, &destination.path).await {
            return error::io_error(&error, 404);
        }
        if method == DavMethod::Move
            && let Err(error) = fs_ops::remove(&source, &source_metadata).await
        {
            return error::io_error(&error, 500);
        }
        return Response::empty(201);
    }

    if method == DavMethod::Move {
        if tokio::fs::rename(&source, &destination.path).await.is_ok() {
            return Response::empty(204);
        }
        if let Err(error) = fs_ops::copy(&source, &destination.path).await {
            return error::io_error(&error, 404);
        }
        if let Err(error) = fs_ops::remove(&source, &source_metadata).await {
            return error::io_error(&error, 500);
        }
        return Response::empty(204);
    }

    match fs_ops::copy(&source, &destination.path).await {
        Ok(()) => Response::empty(204),
        Err(error) => error::io_error(&error, 404),
    }
}
