use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

const V1_SEQUENCER_START: u32 = 1;
const V2_SEQUENCER_START: u32 = 0x0303_0302;

pub(in crate::server) fn prepare_userid_variables(
    context: &EffectiveConfig,
    variables: &mut Variables,
) {
    let config = &context.userid;
    let name_changed = variables.userid.name != config.name;
    let config_changed = name_changed
        || variables.userid.enable != config.enable
        || variables.userid.service != config.service
        || variables.userid.mark != config.mark;
    if name_changed {
        variables.userid.name = config.name.clone();
        variables.userid.got = [0; 4];
        variables.userid.set = [0; 4];
        variables.userid.cookie = find_userid_cookie(&variables.headers, &config.name);
        variables.userid.got = variables
            .userid
            .cookie
            .as_deref()
            .and_then(decode_userid)
            .unwrap_or([0; 4]);
    } else if config_changed {
        variables.userid.set = [0; 4];
    }
    variables.userid.enable = config.enable;
    variables.userid.service = config.service;
    variables.userid.mark = config.mark;
    variables.userid.reset = uid_reset_requested(variables);

    if config.enable == UserIdEnable::Off {
        variables.userid.got = [0; 4];
        variables.userid.set = [0; 4];
        return;
    }
    if config.enable == UserIdEnable::Log {
        variables.userid.set = [0; 4];
        return;
    }

    if variables.userid.got[3] != 0 && !variables.userid.reset {
        if userid_mark_matches(variables.userid.cookie.as_deref(), config.mark) {
            variables.userid.set = [0; 4];
        } else if variables.userid.set[3] == 0 {
            variables.userid.set = variables.userid.got;
        }
    } else if variables.userid.set[3] == 0 {
        variables.userid.set = create_userid(config, &variables.server_addr);
    }
}

fn find_userid_cookie(headers: &[Header], name: &str) -> Option<String> {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("cookie"))
        .flat_map(|header| header.value.split(';'))
        .find_map(|cookie| {
            let (cookie_name, value) = cookie.trim().split_once('=')?;
            cookie_name
                .eq_ignore_ascii_case(name)
                .then(|| value.to_string())
        })
}

fn decode_userid(cookie: &str) -> Option<[u32; 4]> {
    let encoded = cookie.get(..22)?;
    let bytes = base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(encoded)
        .ok()?;
    if bytes.len() != 16 {
        return None;
    }
    let mut uid = [0; 4];
    for (index, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
        uid[index] = u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Some(uid)
}

fn userid_mark_matches(cookie: Option<&str>, mark: Option<u8>) -> bool {
    let Some(mark) = mark else {
        return true;
    };
    let Some(cookie) = cookie else {
        return false;
    };
    cookie.as_bytes().get(22) == Some(&mark) && cookie.as_bytes().get(23) == Some(&b'=')
}

fn uid_reset_requested(variables: &Variables) -> bool {
    variables
        .custom
        .get("uid_reset")
        .is_some_and(|value| !value.is_empty() && value != "0")
}

fn create_userid(config: &UserIdConfig, server_addr: &str) -> [u32; 4] {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let start = userid_start_value();
    match config.enable {
        UserIdEnable::V1 => {
            static SEQUENCER: AtomicU32 = AtomicU32::new(V1_SEQUENCER_START);
            let sequence = SEQUENCER.fetch_add(0x100, Ordering::Relaxed);
            [config.service.unwrap_or(0), now, start, sequence]
        }
        UserIdEnable::On => {
            static SEQUENCER: AtomicU32 = AtomicU32::new(V2_SEQUENCER_START);
            let sequence = SEQUENCER.fetch_add(0x100, Ordering::Relaxed);
            let next = sequence.wrapping_add(0x100);
            if next < V2_SEQUENCER_START {
                SEQUENCER.store(V2_SEQUENCER_START, Ordering::Relaxed);
            }
            [
                config
                    .service
                    .map(u32::to_be)
                    .unwrap_or_else(|| local_address_uid(server_addr)),
                now.to_be(),
                start.to_be(),
                sequence.to_be(),
            ]
        }
        UserIdEnable::Off | UserIdEnable::Log => [0; 4],
    }
}

fn userid_start_value() -> u32 {
    static START: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *START.get_or_init(|| {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_micros();
        ((micros / 20) << 16) | (std::process::id() & 0xffff)
    })
}

fn local_address_uid(value: &str) -> u32 {
    let Ok(address) = value.parse::<IpAddr>() else {
        return 0;
    };
    match address {
        IpAddr::V4(address) => u32::from_ne_bytes(address.octets()),
        IpAddr::V6(address) => {
            let octets = address.octets();
            u32::from_ne_bytes([octets[12], octets[13], octets[14], octets[15]])
        }
    }
}
