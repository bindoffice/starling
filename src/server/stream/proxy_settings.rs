use super::*;

#[derive(Debug, Clone, Copy)]
pub(in crate::server) struct StreamProxySettings {
    pub(in crate::server) upload_rate: u64,
    pub(in crate::server) download_rate: u64,
    pub(in crate::server) buffer_size: usize,
    pub(in crate::server) half_close: bool,
}

impl StreamProxySettings {
    pub(in crate::server) fn from_server(
        server: &crate::config::StreamServer,
        log: &StreamLogContext,
    ) -> Self {
        Self {
            upload_rate: evaluate_rate(server.proxy_upload_rate.as_deref(), log),
            download_rate: evaluate_rate(server.proxy_download_rate.as_deref(), log),
            buffer_size: server.proxy_buffer_size,
            half_close: server.proxy_half_close,
        }
    }
}

pub(in crate::server) fn stream_rate_delay(bytes: usize, rate: u64) -> Option<Duration> {
    if bytes == 0 || rate == 0 {
        return None;
    }
    let delay = Duration::from_secs_f64(bytes as f64 / rate as f64);
    (!delay.is_zero()).then_some(delay)
}

pub(in crate::server) fn evaluate_stream_proxy_bind(
    server: &crate::config::StreamServer,
    log: &StreamLogContext,
) -> Option<String> {
    server
        .proxy_bind
        .as_deref()
        .map(|template| eval_stream_log_variables(template, log))
}

fn evaluate_rate(template: Option<&str>, log: &StreamLogContext) -> u64 {
    let Some(template) = template else {
        return 0;
    };
    let value = eval_stream_log_variables(template, log);
    parse_size_value(&value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_delays_from_packet_size_and_rate() {
        assert_eq!(stream_rate_delay(1024, 1024), Some(Duration::from_secs(1)));
        assert_eq!(
            stream_rate_delay(512, 1024),
            Some(Duration::from_millis(500))
        );
        assert_eq!(stream_rate_delay(1024, 0), None);
        assert_eq!(stream_rate_delay(0, 1024), None);
    }
}
