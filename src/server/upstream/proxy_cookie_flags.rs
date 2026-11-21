use super::*;

const SECURE_ON: u16 = 1 << 0;
const SECURE_OFF: u16 = 1 << 1;
const HTTPONLY_ON: u16 = 1 << 2;
const HTTPONLY_OFF: u16 = 1 << 3;
const SAMESITE_STRICT: u16 = 1 << 4;
const SAMESITE_LAX: u16 = 1 << 5;
const SAMESITE_NONE: u16 = 1 << 6;
const SAMESITE_OFF: u16 = 1 << 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::server) struct ProxyCookieAttribute {
    pub(in crate::server) key: String,
    pub(in crate::server) value: Option<String>,
    pub(in crate::server) removed: bool,
}

pub(in crate::server) fn parse_proxy_cookie(value: &str) -> Vec<ProxyCookieAttribute> {
    value
        .split(';')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            let (key, value) = match part.split_once('=') {
                Some((key, value)) => (key.trim(), Some(value.trim().to_string())),
                None => (part, None),
            };
            Some(ProxyCookieAttribute {
                key: key.to_string(),
                value,
                removed: false,
            })
        })
        .collect()
}

pub(in crate::server) fn render_proxy_cookie(attributes: &[ProxyCookieAttribute]) -> String {
    attributes
        .iter()
        .filter(|attribute| !attribute.removed)
        .map(|attribute| match &attribute.value {
            Some(value) => format!("{}={value}", attribute.key),
            None => attribute.key.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

pub(in crate::server) fn apply_proxy_cookie_flags(
    attributes: &mut Vec<ProxyCookieAttribute>,
    rules: &[ProxyCookieFlagsRule],
    variables: &Variables,
) -> bool {
    let Some(cookie_name) = attributes.first().map(|attribute| attribute.key.as_str()) else {
        return false;
    };
    let Some(rule) = rules.iter().find(|rule| {
        rule.regex.as_ref().map_or_else(
            || eval_variables(&rule.cookie, variables).eq_ignore_ascii_case(cookie_name),
            |regex| regex.is_match(cookie_name),
        )
    }) else {
        return false;
    };

    let flags = rule.flags.iter().fold(0, |flags, flag| {
        flags | parse_proxy_cookie_flag(&eval_variables(flag, variables))
    });
    if flags == 0 {
        return false;
    }

    edit_proxy_cookie_flags(attributes, flags)
}

fn parse_proxy_cookie_flag(value: &str) -> u16 {
    if value.eq_ignore_ascii_case("secure") {
        SECURE_ON
    } else if value.eq_ignore_ascii_case("nosecure") {
        SECURE_OFF
    } else if value.eq_ignore_ascii_case("httponly") {
        HTTPONLY_ON
    } else if value.eq_ignore_ascii_case("nohttponly") {
        HTTPONLY_OFF
    } else if value.eq_ignore_ascii_case("samesite=strict") {
        SAMESITE_STRICT
    } else if value.eq_ignore_ascii_case("samesite=lax") {
        SAMESITE_LAX
    } else if value.eq_ignore_ascii_case("samesite=none") {
        SAMESITE_NONE
    } else if value.eq_ignore_ascii_case("nosamesite") {
        SAMESITE_OFF
    } else {
        0
    }
}

fn edit_proxy_cookie_flags(attributes: &mut Vec<ProxyCookieAttribute>, mut flags: u16) -> bool {
    for attribute in attributes.iter_mut().skip(1) {
        if attribute.removed {
            continue;
        }

        if attribute.key.eq_ignore_ascii_case("secure") {
            if flags & SECURE_ON != 0 {
                flags &= !SECURE_ON;
            } else if flags & SECURE_OFF != 0 {
                attribute.removed = true;
            }
            continue;
        }

        if attribute.key.eq_ignore_ascii_case("httponly") {
            if flags & HTTPONLY_ON != 0 {
                flags &= !HTTPONLY_ON;
            } else if flags & HTTPONLY_OFF != 0 {
                attribute.removed = true;
            }
            continue;
        }

        if !attribute.key.eq_ignore_ascii_case("samesite") {
            continue;
        }

        if flags & SAMESITE_STRICT != 0 {
            flags &= !SAMESITE_STRICT;
            normalize_samesite(attribute, "Strict");
        } else if flags & SAMESITE_LAX != 0 {
            flags &= !SAMESITE_LAX;
            normalize_samesite(attribute, "Lax");
        } else if flags & SAMESITE_NONE != 0 {
            flags &= !SAMESITE_NONE;
            normalize_samesite(attribute, "None");
        } else if flags & SAMESITE_OFF != 0 {
            attribute.removed = true;
        }
    }

    if flags & SECURE_ON != 0 {
        attributes.push(ProxyCookieAttribute {
            key: "Secure".into(),
            value: None,
            removed: false,
        });
    }
    if flags & HTTPONLY_ON != 0 {
        attributes.push(ProxyCookieAttribute {
            key: "HttpOnly".into(),
            value: None,
            removed: false,
        });
    }
    if flags & (SAMESITE_STRICT | SAMESITE_LAX | SAMESITE_NONE) != 0 {
        let value = if flags & SAMESITE_STRICT != 0 {
            "Strict"
        } else if flags & SAMESITE_LAX != 0 {
            "Lax"
        } else {
            "None"
        };
        attributes.push(ProxyCookieAttribute {
            key: "SameSite".into(),
            value: Some(value.into()),
            removed: false,
        });
    }

    true
}

fn normalize_samesite(attribute: &mut ProxyCookieAttribute, value: &str) {
    if attribute
        .value
        .as_deref()
        .is_some_and(|current| current.eq_ignore_ascii_case(value))
    {
        return;
    }
    attribute.key = "SameSite".into();
    attribute.value = Some(value.into());
}
