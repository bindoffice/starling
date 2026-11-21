use super::*;

mod copy_move;
mod delete;
mod error;
mod fs_ops;
mod mkcol;
mod path;
mod put;

pub(in crate::server) async fn serve_dav(
    request: &Request,
    env: &RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
) -> Option<Response> {
    let method = match request.method.as_str() {
        "PUT" => DavMethod::Put,
        "DELETE" => DavMethod::Delete,
        "MKCOL" => DavMethod::MkCol,
        "COPY" => DavMethod::Copy,
        "MOVE" => DavMethod::Move,
        _ => return None,
    };
    if !context.dav.allows(method) {
        return None;
    }
    Some(match method {
        DavMethod::Put => put::handle(request, env, context, location).await,
        DavMethod::Delete => delete::handle(request, env, context, location).await,
        DavMethod::MkCol => mkcol::handle(request, env, context, location).await,
        DavMethod::Copy => {
            copy_move::handle(request, env, context, location, DavMethod::Copy).await
        }
        DavMethod::Move => {
            copy_move::handle(request, env, context, location, DavMethod::Move).await
        }
    })
}
