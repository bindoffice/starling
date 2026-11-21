use super::*;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use md5::{Digest, Md5};

use crate::config::SecureLinkConfig;

pub(in crate::server) fn refresh_secure_link(config: &SecureLinkConfig, variables: &mut Variables) {
    variables.secure_link.clear();
    variables.secure_link_expires.clear();

    if let Some(secret) = config.secret.as_deref() {
        let request_target = variables.request_target.clone();
        refresh_old_secure_link(secret, &request_target, variables);
        return;
    }
    let (Some(source), Some(md5_source)) = (config.variable.as_deref(), config.md5.as_deref())
    else {
        return;
    };

    let source = eval_variables(source, variables);
    let (candidate, expiration) = match source.split_once(',') {
        Some((candidate, expires_text)) => {
            let Ok(expires) = expires_text.parse::<u64>() else {
                return;
            };
            if expires == 0 {
                return;
            }
            // Nginx exposes the original expiration text even when the hash fails.
            variables.secure_link_expires = expires_text.to_string();
            (candidate, Some(expires))
        }
        None => (source.as_str(), None),
    };
    if candidate.len() > 24 {
        return;
    }
    let Some(candidate) = decode_hash(candidate) else {
        return;
    };

    let expected_source = eval_variables(md5_source, variables);
    let expected = Md5::digest(expected_source.as_bytes());
    if !constant_time_eq(&candidate, &expected) {
        return;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    variables.secure_link = if expiration.is_some_and(|expires| expires < now) {
        "0"
    } else {
        "1"
    }
    .into();
}

fn refresh_old_secure_link(secret: &str, request_target: &str, variables: &mut Variables) {
    let Some(rest) = request_target.strip_prefix('/') else {
        return;
    };
    let Some((_, rest)) = rest.split_once('/') else {
        return;
    };
    let Some((hash, path)) = rest.split_once('/') else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Some(hash) = decode_hex_hash(hash) else {
        return;
    };

    let mut hasher = Md5::new();
    hasher.update(path.as_bytes());
    hasher.update(secret.as_bytes());
    let expected = hasher.finalize();
    if constant_time_eq(&hash, &expected) {
        variables.secure_link = path.to_string();
    }
}

fn decode_hash(value: &str) -> Option<[u8; 16]> {
    let decoded = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| URL_SAFE.decode(value))
        .ok()?;
    decoded.try_into().ok()
}

fn decode_hex_hash(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut hash = [0_u8; 16];
    for (index, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(hash)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}
