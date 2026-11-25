use super::*;

mod access;
mod auth;
mod autoindex;
mod buffering;
mod cache;
mod cache_path;
mod cache_shared;
mod cgi;
mod charset;
mod content;
mod dav;
mod disable_symlinks;
mod early_hints;
mod error_log;
mod error_pages;
mod filters;
mod gateway_cache;
mod grpc;
mod gzip;
mod headers;
mod http2;
mod keepalive;
mod lingering;
mod max_headers;
mod msie;
mod open_file_cache;
mod open_log_file_cache;
mod post_action;
mod proxy;
mod proxy_cookie_flags;
mod proxy_hash;
mod ranges;
mod real_ip;
mod request_body;
mod request_headers;
mod rewrite;
mod server_tokens;
mod stapling;
mod store;
mod tls;
mod tunnel;
mod upstream_headers;

use access::try_apply_access_directive;
use auth::try_apply_auth_directive;
use autoindex::try_apply_autoindex_directive;
pub(in crate::config::compile) use buffering::validate_upstream_buffering;
use buffering::{BufferingSeen, try_apply_upstream_buffering_directive};
use cache::ProxyCacheSeen;
use cache::try_apply_proxy_cache_directive;
pub(in crate::config::compile) use cache::validate_cache_zones;
pub(in crate::config::compile) use cache_path::compile_cache_path;
use cgi::try_apply_cgi_directive;
pub(super) use charset::try_apply_charset_directive;
pub(in crate::config::compile) use charset::{
    CharsetSeen, compile_charset_map, validate_charset_map, validate_charset_values,
};
use content::try_apply_content_directive;
use dav::{DavSeen, try_apply_dav_directive};
use disable_symlinks::try_apply_disable_symlinks_directive;
use early_hints::try_apply_early_hints_directive;
pub(in crate::config::compile) use error_log::try_apply_error_log_directive;
use error_pages::try_apply_error_page_directive;
use filters::try_apply_filter_directive;
use gateway_cache::{GatewayCacheSeen, try_apply_gateway_cache_directive};
use grpc::try_apply_grpc_directive;
use gzip::try_apply_gzip_directive;
use headers::try_apply_header_directive;
use http2::{Http2Seen, try_apply_http2_directive};
use keepalive::try_apply_keepalive_directive;
use lingering::try_apply_lingering_directive;
use max_headers::try_apply_max_headers_directive;
use msie::try_apply_msie_directive;
pub(in crate::config::compile) use open_file_cache::try_apply_open_file_cache_directive;
pub(in crate::config::compile) use open_log_file_cache::try_apply_open_log_file_cache_directive;
use post_action::try_apply_post_action_directive;
use proxy::try_apply_proxy_directive;
pub(in crate::config::compile) use proxy::{
    ProxyCookieKind, apply_proxy_cookie, apply_proxy_redirect,
};
pub(in crate::config::compile) use proxy_cookie_flags::apply_proxy_cookie_flags;
use proxy_hash::try_apply_proxy_hash_directive;
use ranges::try_apply_range_directive;
use real_ip::try_apply_real_ip_directive;
pub(in crate::config::compile) use real_ip::{apply_set_real_ip_from, parse_real_ip_source};
use request_body::{RequestBodySeen, try_apply_request_body_directive};
use request_headers::try_apply_request_headers_directive;
use rewrite::try_apply_rewrite_directive;
use server_tokens::try_apply_server_tokens_directive;
pub(in crate::config::compile) use stapling::parse_resolver;
pub(in crate::config::compile) use stapling::{
    StaplingSeen, try_apply_stapling_directive, validate_ssl_stapling,
};
use store::{apply_store_access, apply_store_value};
use tls::try_apply_tls_directive;
pub(in crate::config::compile) use tunnel::parse_tunnel_pass;
use tunnel::{TunnelSeen, try_apply_tunnel_directive};
pub(super) use upstream_headers::{apply_header_list, apply_ignore_headers};

#[derive(Debug, Clone, Copy)]
pub(super) enum Scope {
    Server,
    Location,
}

#[derive(Default)]
pub(super) struct SeenScopeDirectives {
    addition: AdditionSeen,
    pub(super) error_log: bool,
    pub(super) add_headers: Vec<AddHeader>,
    pub(super) add_header_inherit: bool,
    pub(super) add_trailers: Vec<AddHeader>,
    pub(super) add_trailer_inherit: bool,
    expires: bool,
    if_modified_since: bool,
    gzip_disable: bool,
    keepalive_disable: bool,
    keepalive_min_timeout: bool,
    max_ranges: bool,
    max_headers: bool,
    request_body: RequestBodySeen,
    client_header_buffer_size: bool,
    large_client_header_buffers: bool,
    ignore_invalid_headers: bool,
    underscores_in_headers: bool,
    lingering_close: bool,
    lingering_time: bool,
    lingering_timeout: bool,
    early_hints: bool,
    post_action: bool,
    reset_timedout_connection: bool,
    pub(super) tcp_nodelay: bool,
    server_tokens: bool,
    sub_filter: bool,
    ssi: SsiSeen,
    userid: UserIdSeen,
    proxy_header: bool,
    proxy_headers_hash_max_size: bool,
    proxy_headers_hash_bucket_size: bool,
    proxy_hide_header: bool,
    proxy_pass_header: bool,
    proxy_ignore_headers: bool,
    proxy_cookie_domain: bool,
    proxy_cookie_path: bool,
    proxy_cookie_flags: bool,
    proxy_request_buffering: bool,
    proxy_ignore_client_abort: bool,
    tunnel_pass: bool,
    proxy_store: bool,
    proxy_store_access: bool,
    cache: ProxyCacheSeen,
    gateway_cache: GatewayCacheSeen,
    buffering: BufferingSeen,
    fastcgi_store: bool,
    fastcgi_store_access: bool,
    real_ip_from: bool,
    access: bool,
    access_log: bool,
    pub(super) open_log_file_cache: bool,
    auth_delay: bool,
    autoindex: bool,
    autoindex_exact_size: bool,
    autoindex_localtime: bool,
    autoindex_format: bool,
    proxy_redirect: bool,
    limit_req: bool,
    limit_conn: bool,
    fastcgi_param: bool,
    fastcgi_catch_stderr: bool,
    fastcgi_hide_header: bool,
    fastcgi_pass_header: bool,
    fastcgi_ignore_headers: bool,
    fastcgi_ignore_client_abort: bool,
    uwsgi_param: bool,
    uwsgi_string: bool,
    uwsgi_hide_header: bool,
    uwsgi_pass_header: bool,
    uwsgi_ignore_headers: bool,
    uwsgi_ignore_client_abort: bool,
    uwsgi_store: bool,
    uwsgi_store_access: bool,
    scgi_param: bool,
    scgi_hide_header: bool,
    scgi_pass_header: bool,
    scgi_ignore_headers: bool,
    scgi_ignore_client_abort: bool,
    scgi_store: bool,
    scgi_store_access: bool,
    grpc: GrpcSeen,
    ssl_protocols: bool,
    proxy_ssl_protocols: bool,
    pub(super) uwsgi_ssl_protocols: bool,
    valid_referers: bool,
    error_page: bool,
    dav: DavSeen,
    disable_symlinks: bool,
    msie_padding: bool,
    msie_refresh: bool,
    recursive_error_pages: bool,
    mirror: MirrorSeen,
    pub(super) secure_link: SecureLinkSeen,
    browser: BrowserSeen,
    http2: Http2Seen,
    charset: CharsetSeen,
    pub(super) stapling: StaplingSeen,
    tunnel: TunnelSeen,
}

pub(super) fn apply_http_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    secure_link_seen: &mut SecureLinkSeen,
    browser_seen: &mut BrowserSeen,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<()> {
    if try_apply_upstream_buffering_directive(directive, context, prefix, &mut seen.buffering)? {
        return Ok(());
    }
    if try_apply_proxy_cache_directive(directive, context, &mut seen.cache)? {
        return Ok(());
    }
    if try_apply_gateway_cache_directive(directive, context, &mut seen.gateway_cache)? {
        return Ok(());
    }
    if try_apply_grpc_directive(directive, context, prefix, false, &mut seen.grpc)? {
        return Ok(());
    }
    if try_apply_http2_directive(directive, context, None, seen)? {
        return Ok(());
    }
    if try_apply_charset_directive(directive, &mut context.charset, &mut seen.charset)? {
        return Ok(());
    }
    if try_apply_early_hints_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_post_action_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_autoindex_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_auth_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_secure_link_directive(directive, context, secure_link_seen)? {
        return Ok(());
    }
    if try_apply_browser_directive(directive, context, browser_seen)? {
        return Ok(());
    }
    if try_apply_gzip_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_keepalive_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_max_headers_directive(directive, context, seen, true)? {
        return Ok(());
    }
    if try_apply_request_headers_directive(directive, context, None, seen)? {
        return Ok(());
    }
    if try_apply_request_body_directive(directive, context, prefix, &mut seen.request_body)? {
        return Ok(());
    }
    if try_apply_lingering_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_server_tokens_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_range_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_dav_directive(directive, context, &mut seen.dav)? {
        return Ok(());
    }
    if try_apply_disable_symlinks_directive(directive, context, &mut seen.disable_symlinks)? {
        return Ok(());
    }
    if try_apply_tunnel_directive(directive, context, &mut seen.tunnel)? {
        return Ok(());
    }
    if try_apply_msie_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_error_page_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_cgi_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_proxy_directive(directive, context, prefix, seen)? {
        return Ok(());
    }
    apply_common_directive(directive, context, prefix, true, seen)
}

pub(super) fn apply_scope_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    scope: Scope,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<()> {
    if try_apply_error_log_directive(directive, context, prefix, &mut seen.error_log)? {
        return Ok(());
    }
    if try_apply_upstream_buffering_directive(directive, context, prefix, &mut seen.buffering)? {
        return Ok(());
    }
    if try_apply_proxy_cache_directive(directive, context, &mut seen.cache)? {
        return Ok(());
    }
    if try_apply_gateway_cache_directive(directive, context, &mut seen.gateway_cache)? {
        return Ok(());
    }
    if try_apply_grpc_directive(
        directive,
        context,
        prefix,
        matches!(scope, Scope::Location),
        &mut seen.grpc,
    )? {
        return Ok(());
    }
    if try_apply_http2_directive(directive, context, Some(scope), seen)? {
        return Ok(());
    }
    if try_apply_charset_directive(directive, &mut context.charset, &mut seen.charset)? {
        return Ok(());
    }
    if try_apply_early_hints_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_post_action_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_autoindex_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_auth_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_secure_link_directive(directive, context, &mut seen.secure_link)? {
        return Ok(());
    }
    if try_apply_browser_directive(directive, context, &mut seen.browser)? {
        return Ok(());
    }
    if is_userid_directive(&directive.name) {
        apply_userid_directive(directive, context, &mut seen.userid)?;
        return Ok(());
    }
    if is_ssi_directive(&directive.name) {
        apply_ssi_directive(directive, context, &mut seen.ssi)?;
        return Ok(());
    }

    if try_apply_gzip_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_keepalive_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_max_headers_directive(directive, context, seen, matches!(scope, Scope::Server))? {
        return Ok(());
    }
    if try_apply_request_headers_directive(directive, context, Some(scope), seen)? {
        return Ok(());
    }
    if try_apply_request_body_directive(directive, context, prefix, &mut seen.request_body)? {
        return Ok(());
    }
    if try_apply_lingering_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_server_tokens_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_range_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_filter_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_tls_directive(directive, context, prefix, scope, seen)? {
        return Ok(());
    }
    if try_apply_stapling_directive(directive, context, prefix, scope, &mut seen.stapling)? {
        return Ok(());
    }
    if try_apply_header_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_proxy_directive(directive, context, prefix, seen)? {
        return Ok(());
    }
    if try_apply_real_ip_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_access_directive(directive, context, prefix, scope, seen)? {
        return Ok(());
    }
    if try_apply_dav_directive(directive, context, &mut seen.dav)? {
        return Ok(());
    }
    if try_apply_disable_symlinks_directive(directive, context, &mut seen.disable_symlinks)? {
        return Ok(());
    }
    if try_apply_tunnel_directive(directive, context, &mut seen.tunnel)? {
        return Ok(());
    }
    if try_apply_msie_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_error_page_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_rewrite_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_cgi_directive(directive, context, seen)? {
        return Ok(());
    }
    if try_apply_content_directive(directive, context, scope)? {
        return Ok(());
    }

    match directive.name.as_str() {
        "location" => {
            return Err(ConfigError::at(
                directive.location.clone(),
                "location is not allowed in this context",
            ));
        }
        "if" | "limit_except" => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("\"{}\" is not implemented yet", directive.name),
            ));
        }
        _ => {}
    }
    apply_common_directive(
        directive,
        context,
        prefix,
        matches!(scope, Scope::Server),
        seen,
    )
}

pub(super) fn finalize_scope_directives(
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) {
    context.add_header_inherit.merge(
        &mut context.add_headers,
        std::mem::take(&mut seen.add_headers),
    );
    context.add_trailer_inherit.merge(
        &mut context.add_trailers,
        std::mem::take(&mut seen.add_trailers),
    );
}
