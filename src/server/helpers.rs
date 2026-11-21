use super::*;

pub(in crate::server) fn log_access(
    http: &HttpConfig,
    context: &EffectiveConfig,
    request: &Request,
    variables: &Variables,
    response: &Response,
) {
    let config = &context.access_log;
    if config.off {
        return;
    }
    for log in &config.logs {
        if let Some(condition) = &log.condition
            && !log_filter_allows(&eval_variables(condition, variables))
        {
            continue;
        }
        let format = http
            .log_formats
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&log.format_name))
            .map(|(_, format)| format);
        let (template, escape) = format.map_or_else(
            || {
                (
                    "$remote_addr - $remote_user [$time_local] \"$request\" $status $body_bytes_sent \"$http_referer\" \"$http_user_agent\" \"$http_x_forwarded_for\"",
                    LogEscapeMode::Default,
                )
            },
            |format| (format.template.as_str(), format.escape),
        );
        let mut stack = Vec::new();
        let mut line = render_log_format(template, escape, |name| {
            variable_value(name, variables, &mut stack)
        });
        line.push('\n');
        write_access_log_target(
            log,
            &line,
            context.open_log_file_cache.as_ref(),
            |template| eval_variables(template, variables),
        );
    }
    let _ = (request, response);
}

pub(in crate::server) fn write_access_log_target(
    target: &AccessLogTarget,
    line: &str,
    file_cache: Option<&Arc<OpenLogFileCacheConfig>>,
    evaluate: impl FnOnce(&str) -> String,
) {
    match &target.destination {
        AccessLogDestination::File(AccessLogPath::Fixed(path)) => {
            append_access_log(Some(path), line, target.buffer.as_ref(), file_cache);
        }
        AccessLogDestination::File(AccessLogPath::Dynamic(template)) => {
            let path = evaluate(template);
            if !path.is_empty() {
                append_access_log(
                    Some(Path::new(&path)),
                    line,
                    target.buffer.as_ref(),
                    file_cache,
                );
            }
        }
        AccessLogDestination::Syslog(config) => send_syslog(config, line),
    }
}

pub(in crate::server) fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(in crate::server) fn percent_encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

pub(in crate::server) fn request_length(request: &Request) -> u64 {
    let line = request.method.len() + request.target.len() + request.version.len() + 4; // two spaces and CRLF
    let headers = request
        .headers
        .iter()
        .map(|header| header.name.len() + header.value.len() + 4) // ": " and CRLF
        .sum::<usize>();
    line.saturating_add(headers)
        .saturating_add(2) // final CRLF
        .saturating_add(request.buffered_body_len() as usize) as u64
}

pub(in crate::server) fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            #[cfg(unix)]
            {
                let mut buffer = [0_u8; 256];
                let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
                if result == 0 {
                    let length = buffer
                        .iter()
                        .position(|byte| *byte == 0)
                        .unwrap_or(buffer.len());
                    return String::from_utf8_lossy(&buffer[..length]).into_owned();
                }
            }
            "localhost".into()
        })
}

pub(in crate::server) fn next_request_id() -> String {
    static REQUEST_ID: AtomicU64 = AtomicU64::new(1);
    format!(
        "{:016x}{:016x}",
        std::process::id(),
        REQUEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}

pub(in crate::server) fn iso_timestamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::gmtime_r(&seconds, &mut value);
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}+00:00",
        value.tm_year + 1900,
        value.tm_mon + 1,
        value.tm_mday,
        value.tm_hour,
        value.tm_min,
        value.tm_sec
    )
}
