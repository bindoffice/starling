use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use starling::config::{
    AutoHttpsChallenge, AutoindexFormat, ClientCertificateVerify, ConfigLoader, DavMethod,
    DisableSymlinksMode, ExpiresOffset, ExpiresValue, GzipHttpVersion, GzipStatic, IfModifiedSince,
    LingeringClose, ListenAddress, ListenKeepalive, ListenSocketOptions, LogDestination,
    LogEscapeMode, LogLevel, OcspMode, ProxyStoreConfig, ServerTokens, StoreConfig, SyslogServer,
    TlsCipherSuite, compile,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[path = "config_compat/access_auth.rs"]
mod access_auth;
#[path = "config_compat/access_limits.rs"]
mod access_limits;
#[path = "config_compat/access_logs.rs"]
mod access_logs;
#[path = "config_compat/auto_https.rs"]
mod auto_https;
#[path = "config_compat/autoindex.rs"]
mod autoindex;
#[path = "config_compat/basic.rs"]
mod basic;
#[path = "config_compat/cache.rs"]
mod cache;
#[path = "config_compat/charset.rs"]
mod charset;
#[path = "config_compat/dav.rs"]
mod dav;
#[path = "config_compat/disable_symlinks.rs"]
mod disable_symlinks;
#[path = "config_compat/early_hints.rs"]
mod early_hints;
#[path = "config_compat/error_log.rs"]
mod error_log;
#[path = "config_compat/error_pages.rs"]
mod error_pages;
#[path = "config_compat/gateway_cache.rs"]
mod gateway_cache;
#[path = "config_compat/gateway_headers.rs"]
mod gateway_headers;
#[path = "config_compat/gateway_params.rs"]
mod gateway_params;
#[path = "config_compat/gateway_ranges.rs"]
mod gateway_ranges;
#[path = "config_compat/gateway_stores.rs"]
mod gateway_stores;
#[path = "config_compat/grpc.rs"]
mod grpc;
#[path = "config_compat/gzip.rs"]
mod gzip;
#[path = "config_compat/keepalive.rs"]
mod keepalive;
#[path = "config_compat/lingering.rs"]
mod lingering;
#[path = "config_compat/listen_socket.rs"]
mod listen_socket;
#[path = "config_compat/log_format.rs"]
mod log_format;
#[path = "config_compat/max_headers.rs"]
mod max_headers;
#[path = "config_compat/memcached.rs"]
mod memcached;
#[path = "config_compat/msie.rs"]
mod msie;
#[path = "config_compat/open_file_cache.rs"]
mod open_file_cache;
#[path = "config_compat/open_log_file_cache.rs"]
mod open_log_file_cache;
#[path = "config_compat/post_action.rs"]
mod post_action;
#[path = "config_compat/proxy_cookie_flags.rs"]
mod proxy_cookie_flags;
#[path = "config_compat/proxy_hash.rs"]
mod proxy_hash;
#[path = "config_compat/proxy_http2.rs"]
mod proxy_http2;
#[path = "config_compat/proxy_store.rs"]
mod proxy_store;
#[path = "config_compat/proxy_timeouts.rs"]
mod proxy_timeouts;
#[path = "config_compat/ranges.rs"]
mod ranges;
#[path = "config_compat/real_world.rs"]
mod real_world;
#[path = "config_compat/request_body.rs"]
mod request_body;
#[path = "config_compat/request_headers.rs"]
mod request_headers;
#[path = "config_compat/response.rs"]
mod response;
#[path = "config_compat/rewrite.rs"]
mod rewrite;
#[path = "config_compat/routing.rs"]
mod routing;
#[path = "config_compat/server_tokens.rs"]
mod server_tokens;
#[path = "config_compat/tcp_nodelay.rs"]
mod tcp_nodelay;
#[path = "config_compat/tls_ciphers.rs"]
mod tls_ciphers;
#[path = "config_compat/tls_client.rs"]
mod tls_client;
#[path = "config_compat/tls_conf_commands.rs"]
mod tls_conf_commands;
#[path = "config_compat/tls_ocsp.rs"]
mod tls_ocsp;
#[path = "config_compat/tls_protocols.rs"]
mod tls_protocols;
#[path = "config_compat/tls_stapling.rs"]
mod tls_stapling;
#[path = "config_compat/trailers.rs"]
mod trailers;
#[path = "config_compat/transfer.rs"]
mod transfer;
#[path = "config_compat/tunnel.rs"]
mod tunnel;
#[path = "config_compat/upstream.rs"]
mod upstream;
#[path = "config_compat/upstream_buffering.rs"]
mod upstream_buffering;
#[path = "config_compat/upstream_rates.rs"]
mod upstream_rates;
#[path = "config_compat/upstream_socket.rs"]
mod upstream_socket;
#[path = "config_compat/uwsgi.rs"]
mod uwsgi;
#[path = "config_compat/uwsgi_tls.rs"]
mod uwsgi_tls;
#[path = "config_compat/variables.rs"]
mod variables;

fn temp_prefix() -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("starling-config-test-{}-{id}", std::process::id()))
}
