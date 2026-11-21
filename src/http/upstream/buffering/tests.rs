use super::*;
use crate::config::TempPathConfig;

fn header(value: &str) -> Header {
    Header {
        name: "X-Accel-Buffering".into(),
        value: value.into(),
    }
}

#[test]
fn parses_only_case_insensitive_yes_and_no() {
    assert_eq!(
        parse_accel_buffering(&[header("no")]),
        Some(AccelBuffering::No)
    );
    assert_eq!(
        parse_accel_buffering(&[header("YES")]),
        Some(AccelBuffering::Yes)
    );
    assert_eq!(parse_accel_buffering(&[header("off")]), None);
    assert_eq!(parse_accel_buffering(&[header("no ")]), None);
}

#[test]
fn last_valid_value_wins_and_invalid_values_are_ignored() {
    let headers = [
        header("no"),
        header("invalid"),
        header("YeS"),
        Header {
            name: "X-Other".into(),
            value: "no".into(),
        },
        header("invalid"),
    ];
    assert_eq!(parse_accel_buffering(&headers), Some(AccelBuffering::Yes));
}

#[test]
fn policy_switches_between_configured_memory_and_unbuffered_modes() {
    let config = crate::config::UpstreamBufferingConfig {
        enabled: false,
        buffer_size: 4096,
        buffers: crate::config::UpstreamBuffers {
            count: 2,
            size: 4096,
        },
        busy_buffers_size: None,
        temp_path: TempPathConfig::new(std::env::temp_dir().join("starling-policy")),
        max_temp_file_size: Some(128 * 1024),
        temp_file_write_size: None,
    };
    let policy = UpstreamBodyBufferPolicy::upstream(&config, true);

    let default = policy.options_for_headers(&[]);
    assert_eq!(default.memory_limit, MAX_BODY_BYTES);
    assert_eq!(default.busy_limit, MAX_BODY_BYTES);
    assert!(default.temp_path.is_none());

    let unbuffered = policy.options_for_headers(&[header("no")]);
    assert_eq!(unbuffered.memory_limit, MAX_BODY_BYTES);
    assert_eq!(unbuffered.busy_limit, MAX_BODY_BYTES);
    assert!(unbuffered.temp_path.is_none());

    let buffered = policy.options_for_headers(&[header("yes")]);
    assert_eq!(buffered.memory_limit, 8192);
    assert_eq!(buffered.busy_limit, 8192);
    assert!(buffered.temp_path.is_some());

    let ignored =
        UpstreamBodyBufferPolicy::upstream(&config, false).options_for_headers(&[header("yes")]);
    assert_eq!(ignored.memory_limit, MAX_BODY_BYTES);
    assert_eq!(ignored.busy_limit, MAX_BODY_BYTES);
    assert!(ignored.temp_path.is_none());
}

#[test]
fn options_carry_the_configured_busy_buffer_window() {
    let config = crate::config::UpstreamBufferingConfig {
        enabled: true,
        buffer_size: 4096,
        buffers: crate::config::UpstreamBuffers {
            count: 4,
            size: 4096,
        },
        busy_buffers_size: Some(8192),
        temp_path: TempPathConfig::new(std::env::temp_dir().join("starling-busy-options")),
        max_temp_file_size: Some(128 * 1024),
        temp_file_write_size: None,
    };

    let options = UpstreamBodyBufferOptions::upstream(&config);
    assert_eq!(options.memory_limit, 16 * 1024);
    assert_eq!(options.busy_limit, 8 * 1024);
}
