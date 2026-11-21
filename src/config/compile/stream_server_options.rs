use super::stream_tls_options::StreamTlsOptions;
use super::*;

pub(super) const DEFAULT_STREAM_PROXY_BUFFER_SIZE: usize = 16 * 1024;
pub(super) const DEFAULT_STREAM_PREREAD_BUFFER_SIZE: usize = 16 * 1024;

pub(super) struct StreamServerDefaults {
    pub(super) error_logs: Vec<LogTarget>,
    pub(super) tls: StreamTlsOptions,
    pub(super) access_log: AccessLogConfig,
    pub(super) open_log_file_cache: Option<Arc<OpenLogFileCacheConfig>>,
    pub(super) access_rules: Vec<AccessRule>,
    pub(super) real_ip_from: Vec<RealIpSource>,
    pub(super) tcp_nodelay: bool,
    pub(super) ssl_preread: bool,
    pub(super) preread_timeout: Duration,
    pub(super) preread_buffer_size: usize,
    pub(super) limit_conns: Vec<LimitConnLimit>,
    pub(super) limit_conn_dry_run: bool,
    pub(super) limit_conn_log_level: LogLevel,
    pub(super) proxy_protocol: StreamProxyProtocol,
    pub(super) proxy_protocol_timeout: Duration,
    pub(super) proxy_half_close: bool,
    pub(super) proxy_requests: usize,
    pub(super) proxy_responses: usize,
    pub(super) proxy_next_upstream: bool,
    pub(super) proxy_next_upstream_tries: usize,
    pub(super) proxy_next_upstream_timeout: Duration,
    pub(super) proxy_upload_rate: Option<String>,
    pub(super) proxy_download_rate: Option<String>,
    pub(super) proxy_buffer_size: usize,
    pub(super) proxy_socket_keepalive: bool,
    pub(super) proxy_socket_rcvbuf: usize,
    pub(super) proxy_socket_sndbuf: usize,
    pub(super) proxy_bind: Option<String>,
}

pub(super) fn parse_stream_packet_count(directive: &Directive, name: &str) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    parse_usize(value, &directive.location, name)
}

pub(super) fn parse_stream_rate(directive: &Directive, name: &str) -> ConfigResult<Option<String>> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if !value.contains('$') {
        parse_size(value, &directive.location).map_err(|_| {
            ConfigError::at(
                directive.location.clone(),
                format!("invalid value \"{value}\" in \"{name}\" directive"),
            )
        })?;
    }
    Ok(Some(value.to_string()))
}

pub(super) fn parse_stream_buffer_size(directive: &Directive, name: &str) -> ConfigResult<usize> {
    let size = parse_stream_size(directive, name)?;
    if size == 0 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("value must be nonzero in \"{name}\" directive"),
        ));
    }
    Ok(size)
}

pub(super) fn parse_stream_size(directive: &Directive, name: &str) -> ConfigResult<usize> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })?;
    usize::try_from(size).map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}

pub(super) fn parse_stream_bind(directive: &Directive) -> ConfigResult<Option<String>> {
    validate_no_block(directive)?;
    if !matches!(directive.args.len(), 1 | 2) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"proxy_bind\" directive",
        ));
    }
    let address = &directive.args[0];
    if directive.args.len() == 2 {
        if directive.args[1] != "transparent" {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid parameter \"{}\" in \"proxy_bind\" directive",
                    directive.args[1]
                ),
            ));
        }
        return Err(ConfigError::at(
            directive.location.clone(),
            "transparent proxying is not supported in \"proxy_bind\" directive",
        ));
    }
    if address == "off" {
        return Ok(None);
    }
    Ok(Some(address.clone()))
}

pub(super) fn parse_stream_alpn(directive: &Directive, name: &str) -> ConfigResult<Vec<String>> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }
    if directive.args.iter().any(|protocol| protocol.len() > 255) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("protocol too long in \"{name}\" directive"),
        ));
    }
    Ok(directive.args.clone())
}
