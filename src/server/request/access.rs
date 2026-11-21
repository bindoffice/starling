use super::*;

mod auth_request;
mod basic_auth;
mod limits;
mod location;
mod real_ip;
mod status;

pub(in crate::server) use auth_request::*;
pub(in crate::server) use basic_auth::*;
pub(in crate::server) use limits::*;
pub(in crate::server) use location::*;
pub(in crate::server) use real_ip::*;
pub(in crate::server) use status::*;

pub(in crate::server) async fn apply_auth_delay(
    context: &EffectiveConfig,
    response: Response,
) -> Response {
    if response.status == 401 && context.auth_delay != Duration::ZERO {
        tokio::time::sleep(context.auth_delay).await;
    }
    response
}
