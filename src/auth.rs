#[cfg(unix)]
use std::ffi::CString;
#[cfg(unix)]
use std::sync::Mutex;

use base64::Engine;
use base64::alphabet;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use md5::{Digest as Md5Digest, Md5};
use sha1::Sha1;

const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasicCredentials {
    pub username: String,
    pub password: String,
}

pub fn parse_authorization_header(value: &str) -> Option<BasicCredentials> {
    let encoded = value
        .get(..5)
        .filter(|prefix| prefix.eq_ignore_ascii_case("Basic"))
        .and_then(|_| value.get(5..))?
        .trim_start();
    if encoded.is_empty() {
        return None;
    }
    let decoded = BASE64.decode(encoded).ok()?;
    let separator = decoded.iter().position(|byte| *byte == b':')?;
    if separator == 0 {
        return None;
    }
    let username = std::str::from_utf8(&decoded[..separator]).ok()?;
    let password = std::str::from_utf8(&decoded[separator + 1..]).ok()?;
    Some(BasicCredentials {
        username: username.to_string(),
        password: password.to_string(),
    })
}

pub fn find_user_password<'a>(user_file: &'a str, username: &str) -> Option<&'a str> {
    for line in user_file.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, password)) = line.split_once(':') else {
            continue;
        };
        if name == username {
            return Some(password);
        }
    }
    None
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    if let Some(plain) = stored.strip_prefix("{PLAIN}") {
        return constant_time_eq(password.as_bytes(), plain.as_bytes());
    }
    if let Some(encoded) = stored.strip_prefix("{SHA}") {
        let digest = Sha1::digest(password.as_bytes());
        return encoded_digest_matches(encoded, &digest);
    }
    if let Some(encoded) = stored.strip_prefix("{SSHA}") {
        return verify_ssha(password, encoded);
    }
    if stored.starts_with("$apr1$") {
        return apr1_crypt(password, stored) == stored;
    }
    crypt_password(password, stored).is_some_and(|encrypted| encrypted == stored)
}

fn encoded_digest_matches(encoded: &str, digest: &[u8]) -> bool {
    BASE64
        .decode(encoded)
        .ok()
        .is_some_and(|decoded| constant_time_eq(&decoded, digest))
}

fn verify_ssha(password: &str, encoded: &str) -> bool {
    let Ok(decoded) = BASE64.decode(encoded) else {
        return false;
    };
    if decoded.len() < 20 {
        return false;
    }
    let salt = &decoded[20..];
    let mut hasher = Sha1::new();
    hasher.update(password.as_bytes());
    hasher.update(salt);
    let digest = hasher.finalize();
    let mut expected = digest.to_vec();
    expected.extend_from_slice(salt);
    constant_time_eq(&decoded, &expected)
}

fn apr1_crypt(password: &str, stored: &str) -> String {
    let salt = stored
        .strip_prefix("$apr1$")
        .unwrap_or_default()
        .split('$')
        .next()
        .unwrap_or_default();
    let salt = &salt[..salt.len().min(8)];
    let password = password.as_bytes();
    let salt = salt.as_bytes();

    let mut initial = Md5::new();
    initial.update(password);
    initial.update(b"$apr1$");
    initial.update(salt);

    let mut alternate = Md5::new();
    alternate.update(password);
    alternate.update(salt);
    alternate.update(password);
    let alternate = alternate.finalize();

    let mut remaining = password.len();
    while remaining > 0 {
        initial.update(&alternate[..remaining.min(16)]);
        remaining = remaining.saturating_sub(16);
    }

    let mut index = password.len();
    while index > 0 {
        if index & 1 != 0 {
            initial.update([0_u8]);
        } else {
            initial.update(&password[..1]);
        }
        index >>= 1;
    }

    let mut digest = initial.finalize();
    for round in 0..1000 {
        let mut hasher = Md5::new();
        if round & 1 != 0 {
            hasher.update(password);
        } else {
            hasher.update(digest);
        }
        if round % 3 != 0 {
            hasher.update(salt);
        }
        if round % 7 != 0 {
            hasher.update(password);
        }
        if round & 1 != 0 {
            hasher.update(digest);
        } else {
            hasher.update(password);
        }
        digest = hasher.finalize();
    }

    let mut output = format!("$apr1${}", String::from_utf8_lossy(salt));
    output.push('$');
    output.push_str(&to64(
        (u32::from(digest[0]) << 16) | (u32::from(digest[6]) << 8) | u32::from(digest[12]),
        4,
    ));
    output.push_str(&to64(
        (u32::from(digest[1]) << 16) | (u32::from(digest[7]) << 8) | u32::from(digest[13]),
        4,
    ));
    output.push_str(&to64(
        (u32::from(digest[2]) << 16) | (u32::from(digest[8]) << 8) | u32::from(digest[14]),
        4,
    ));
    output.push_str(&to64(
        (u32::from(digest[3]) << 16) | (u32::from(digest[9]) << 8) | u32::from(digest[15]),
        4,
    ));
    output.push_str(&to64(
        (u32::from(digest[4]) << 16) | (u32::from(digest[10]) << 8) | u32::from(digest[5]),
        4,
    ));
    output.push_str(&to64(u32::from(digest[11]), 2));
    output
}

fn to64(mut value: u32, count: usize) -> String {
    const ITOA64: &[u8; 64] = b"./0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut output = String::with_capacity(count);
    for _ in 0..count {
        output.push(ITOA64[(value & 0x3f) as usize] as char);
        value >>= 6;
    }
    output
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (left, right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(unix)]
fn crypt_password(password: &str, stored: &str) -> Option<String> {
    use std::ffi::CStr;

    static CRYPT_LOCK: Mutex<()> = Mutex::new(());

    #[cfg_attr(all(unix, not(target_vendor = "apple")), link(name = "crypt"))]
    unsafe extern "C" {
        fn crypt(key: *const libc::c_char, salt: *const libc::c_char) -> *const libc::c_char;
    }

    let password = CString::new(password).ok()?;
    let salt = CString::new(stored).ok()?;
    let _guard = CRYPT_LOCK.lock().ok()?;
    // SAFETY: crypt() receives valid NUL-terminated strings and the lock serializes
    // access to its process-global result buffer.
    let encrypted = unsafe { crypt(password.as_ptr(), salt.as_ptr()) };
    if encrypted.is_null() {
        return None;
    }
    // SAFETY: a non-null crypt() result points to a NUL-terminated string.
    let encrypted = unsafe { CStr::from_ptr(encrypted) };
    encrypted.to_str().ok().map(str::to_string)
}

#[cfg(not(unix))]
fn crypt_password(_password: &str, _stored: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_authorization() {
        assert_eq!(
            parse_authorization_header("Basic dXNlcjpwYXNz"),
            Some(BasicCredentials {
                username: "user".into(),
                password: "pass".into(),
            })
        );
        assert_eq!(
            parse_authorization_header("basic dXNlcjpwYXNz"),
            Some(BasicCredentials {
                username: "user".into(),
                password: "pass".into(),
            })
        );
        assert!(parse_authorization_header("Bearer token").is_none());
        assert!(parse_authorization_header("Basic !!!").is_none());
    }

    #[test]
    fn verifies_nginx_supported_password_hashes() {
        assert!(verify_password("secret", "{PLAIN}secret"));
        assert!(verify_password(
            "secret",
            "{SHA}5en6G6MezRroT3XKqkdPOmY/BfQ="
        ));
        assert!(verify_password(
            "secret",
            "{SSHA}Wcm1xEisNjqp921ALcHfuQ7avFdzYWx0MTIzNA=="
        ));
        assert!(verify_password(
            "secret",
            "$apr1$abcdefgh$h9FWgUz3n9YxylKLlR5SQ/"
        ));
        assert!(!verify_password(
            "wrong",
            "$apr1$abcdefgh$h9FWgUz3n9YxylKLlR5SQ/"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn verifies_system_crypt_hashes() {
        assert!(verify_password(
            "secret",
            "$2y$05$abcdefghijklmnopqrstuuOQiyCxlgf/oeuTqixKmWdcYUh4Hjl0a"
        ));
    }

    #[test]
    fn reads_the_first_matching_user_file_entry() {
        let file = "# comment\r\nuser:{PLAIN}first\r\nother:{PLAIN}value\r\n";
        assert_eq!(find_user_password(file, "user"), Some("{PLAIN}first"));
        assert_eq!(find_user_password(file, "missing"), None);
    }
}
