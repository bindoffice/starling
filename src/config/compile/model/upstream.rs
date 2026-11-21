mod balance;
mod server;
mod types;

pub use server::UpstreamPeerGuard;
pub use types::{
    LeastTimeConfig, LeastTimeMode, UpstreamAttemptTiming, UpstreamConfig, UpstreamKeepaliveConfig,
    UpstreamMethod, UpstreamServer,
};
pub(in crate::config::compile) use types::{UpstreamHealth, UpstreamTiming};
