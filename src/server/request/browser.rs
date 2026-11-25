use super::*;
use crate::config::{BrowserConfig, BrowserKind, ModernBrowserRule};

pub(in crate::server) fn refresh_browser_variables(
    config: &BrowserConfig,
    variables: &mut Variables,
) {
    let user_agent = variables
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("user-agent"))
        .map(|header| header.value.as_str());
    let msie = user_agent.is_some_and(is_msie);
    let modern = matches!(classify_browser(config, user_agent), BrowserClass::Modern);
    variables.msie = if msie { "1".into() } else { String::new() };
    variables.modern_browser = if modern {
        config.modern_value.clone()
    } else {
        String::new()
    };
    variables.ancient_browser = if modern {
        String::new()
    } else {
        config.ancient_value.clone()
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BrowserClass {
    Modern,
    Ancient,
}

fn classify_browser(config: &BrowserConfig, user_agent: Option<&str>) -> BrowserClass {
    let Some(user_agent) = user_agent else {
        return if config.modern_unlisted {
            BrowserClass::Modern
        } else {
            BrowserClass::Ancient
        };
    };

    if !config.modern.is_empty() {
        let opera = modern_rule(config, BrowserKind::Opera);
        if opera.is_none() && user_agent.contains("Opera") {
            // Nginx inserts an unreachable Opera rule to prevent MSIE false matches.
            return BrowserClass::Ancient;
        }
        for kind in [
            BrowserKind::Opera,
            BrowserKind::Msie,
            BrowserKind::Gecko,
            BrowserKind::Safari,
            BrowserKind::Konqueror,
        ] {
            let Some(rule) = modern_rule(config, kind) else {
                continue;
            };
            let Some(start) = browser_version_start(kind, user_agent) else {
                continue;
            };
            return if browser_version(user_agent, start) >= rule.version {
                BrowserClass::Modern
            } else {
                BrowserClass::Ancient
            };
        }
        if !config.modern_unlisted {
            return BrowserClass::Ancient;
        }
    }

    if config.netscape4 && is_netscape4(user_agent) {
        return BrowserClass::Ancient;
    }
    if config
        .ancient
        .iter()
        .any(|browser| user_agent.contains(browser))
    {
        return BrowserClass::Ancient;
    }
    if config.modern_unlisted {
        BrowserClass::Modern
    } else {
        BrowserClass::Ancient
    }
}

fn modern_rule(config: &BrowserConfig, kind: BrowserKind) -> Option<&ModernBrowserRule> {
    config.modern.iter().find(|rule| rule.browser == kind)
}

fn browser_version_start(kind: BrowserKind, user_agent: &str) -> Option<usize> {
    let (skip, marker, advance) = match kind {
        BrowserKind::Opera => (0, "Opera", 6),
        BrowserKind::Msie => ("Mozilla/4.0 (compatible; ".len(), "MSIE ", 5),
        BrowserKind::Gecko => ("Mozilla/5.0 (".len(), "rv:", 3),
        BrowserKind::Safari => ("Mozilla/5.0 (".len(), "Safari/", 7),
        BrowserKind::Konqueror => ("Mozilla/5.0 (compatible; ".len(), "Konqueror/", 10),
    };
    let tail = user_agent.get(skip..)?;
    let position = tail.find(marker)?;
    Some(skip + position + advance)
}

fn browser_version(user_agent: &str, start: usize) -> u32 {
    let mut version = 0_u32;
    let mut part = 0_u32;
    let mut scale = 1_000_000_u32;
    for byte in user_agent.as_bytes().iter().skip(start) {
        if byte.is_ascii_digit() {
            part = part
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            continue;
        }
        if *byte == b'.' {
            version = version.saturating_add(part.saturating_mul(scale));
            part = 0;
            scale /= 100;
            continue;
        }
        break;
    }
    version.saturating_add(part.saturating_mul(scale))
}

fn is_netscape4(user_agent: &str) -> bool {
    user_agent.len() > "Mozilla/4.72 ".len()
        && user_agent.starts_with("Mozilla/")
        && user_agent
            .as_bytes()
            .get(8)
            .is_some_and(|version| (b'1'..=b'4').contains(version))
}

fn is_msie(user_agent: &str) -> bool {
    if user_agent.contains("Opera") {
        return false;
    }
    let Some(position) = user_agent.find("MSIE ") else {
        return false;
    };
    position + 7 < user_agent.len()
        && user_agent
            .as_bytes()
            .get(position + 5)
            .is_some_and(u8::is_ascii_digit)
}
