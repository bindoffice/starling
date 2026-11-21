pub mod auth;
pub mod config;
pub mod error;
pub mod http;
pub mod logging;
pub mod runtime;
pub mod server;

pub const SERVER_SOFTWARE: &str = "nginx/1.31.9";
pub const SERVER_SOFTWARE_BUILD: &str =
    concat!("nginx/1.31.9 (starling/", env!("CARGO_PKG_VERSION"), ")");
pub const STARLING_VERSION: &str = env!("CARGO_PKG_VERSION");
