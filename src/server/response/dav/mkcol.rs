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
    if !env.uri.ends_with('/') {
        return error::status(409);
    }

    let path = path::source_path(context, location, &env.uri);
    if let Err(error) = fs_ops::create_dir(&path, context.dav.access).await {
        let status = match error.kind() {
            io::ErrorKind::AlreadyExists => 405,
            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => 409,
            _ => 0,
        };
        return if status == 0 {
            error::io_error(&error, 409)
        } else {
            error::status_response(status)
        };
    }

    let mut response = Response::empty(201);
    response.set_header("Location", path::location_header(&env.uri));
    response
}
