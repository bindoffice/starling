use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccelBuffering {
    No,
    Yes,
}

#[derive(Debug, Clone)]
pub struct UpstreamBodyBufferPolicy {
    default: UpstreamBodyBufferOptions,
    buffered: UpstreamBodyBufferOptions,
    unbuffered: UpstreamBodyBufferOptions,
    honor_x_accel_buffering: bool,
}

impl UpstreamBodyBufferPolicy {
    pub fn upstream(
        config: &crate::config::UpstreamBufferingConfig,
        honor_x_accel_buffering: bool,
    ) -> Self {
        Self {
            default: UpstreamBodyBufferOptions::upstream(config),
            buffered: UpstreamBodyBufferOptions::buffered(config),
            unbuffered: UpstreamBodyBufferOptions::memory(MAX_BODY_BYTES),
            honor_x_accel_buffering,
        }
    }

    pub fn fixed(options: UpstreamBodyBufferOptions) -> Self {
        Self {
            default: options.clone(),
            buffered: options.clone(),
            unbuffered: options,
            honor_x_accel_buffering: false,
        }
    }

    pub fn options_for_headers(&self, headers: &[Header]) -> UpstreamBodyBufferOptions {
        if !self.honor_x_accel_buffering {
            return self.default.clone();
        }
        match parse_accel_buffering(headers) {
            Some(AccelBuffering::No) => self.unbuffered.clone(),
            Some(AccelBuffering::Yes) => self.buffered.clone(),
            None => self.default.clone(),
        }
    }
}

pub fn parse_accel_buffering(headers: &[Header]) -> Option<AccelBuffering> {
    let mut buffering = None;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("X-Accel-Buffering") {
            continue;
        }
        buffering = if header.value.eq_ignore_ascii_case("no") {
            Some(AccelBuffering::No)
        } else if header.value.eq_ignore_ascii_case("yes") {
            Some(AccelBuffering::Yes)
        } else {
            buffering
        };
    }
    buffering
}

#[cfg(test)]
mod tests;
