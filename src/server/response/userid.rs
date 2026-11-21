use super::*;

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MAX_EXPIRES: &str = "; expires=Thu, 31-Dec-37 23:55:55 GMT";

pub(in crate::server) fn apply_userid(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &mut Variables,
) {
    let config = &context.userid;
    if config.enable != UserIdEnable::V1 && config.enable != UserIdEnable::On {
        return;
    }
    prepare_userid_variables(context, variables);
    let state = &variables.userid;
    if state.set[3] == 0 {
        return;
    }

    let cookie_value = if state.got[3] == 0 || state.reset {
        encode_userid(state.set, config.mark)
    } else if let Some(mark) = config.mark {
        let mut cookie = state
            .cookie
            .as_deref()
            .and_then(|cookie| cookie.get(..22))
            .unwrap_or_default()
            .as_bytes()
            .to_vec();
        cookie.push(mark);
        cookie.push(b'=');
        String::from_utf8(cookie).unwrap_or_default()
    } else {
        encode_userid(state.set, None)
    };

    let mut cookie = format!("{}={cookie_value}", config.name);
    match config.expires {
        UserIdExpires::Off => {}
        UserIdExpires::Max => cookie.push_str(MAX_EXPIRES),
        UserIdExpires::Seconds(seconds) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            cookie.push_str("; expires=");
            cookie.push_str(&cookie_time(now.saturating_add(seconds)));
        }
    }
    if let Some(domain) = &config.domain {
        cookie.push_str("; domain=");
        cookie.push_str(domain);
    }
    cookie.push_str("; path=");
    cookie.push_str(&config.path);
    if config.flags.secure {
        cookie.push_str("; secure");
    }
    if config.flags.http_only {
        cookie.push_str("; httponly");
    }
    if config.flags.same_site_strict {
        cookie.push_str("; samesite=strict");
    }
    if config.flags.same_site_lax {
        cookie.push_str("; samesite=lax");
    }
    if config.flags.same_site_none {
        cookie.push_str("; samesite=none");
    }
    response.add_header("Set-Cookie", cookie);
    if let Some(p3p) = &config.p3p {
        response.add_header("P3P", p3p);
    }
}

fn encode_userid(uid: [u32; 4], mark: Option<u8>) -> String {
    let mut bytes = Vec::with_capacity(16);
    for word in uid {
        bytes.extend_from_slice(&word.to_ne_bytes());
    }
    let mut encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    if let Some(mark) = mark
        && encoded.len() > 22
    {
        encoded.replace_range(22..23, &(mark as char).to_string());
    }
    encoded
}

fn cookie_time(timestamp: u64) -> String {
    let days = (timestamp / 86_400) as i64;
    let seconds = timestamp % 86_400;
    let (year, month, day) = civil_from_days(days);
    let weekday = WEEKDAYS[((days + 4).rem_euclid(7)) as usize];
    let hour = seconds / 3_600;
    let minute = seconds % 3_600 / 60;
    let second = seconds % 60;
    if year > 2037 {
        format!(
            "{weekday}, {day:02}-{}-{year} {hour:02}:{minute:02}:{second:02} GMT",
            MONTHS[month as usize - 1]
        )
    } else {
        format!(
            "{weekday}, {day:02}-{}-{:02} {hour:02}:{minute:02}:{second:02} GMT",
            MONTHS[month as usize - 1],
            year % 100
        )
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_nginx_cookie_dates() {
        assert_eq!(cookie_time(0), "Thu, 01-Jan-70 00:00:00 GMT");
        assert_eq!(cookie_time(2_145_916_555), "Thu, 31-Dec-37 23:55:55 GMT");
    }
}
