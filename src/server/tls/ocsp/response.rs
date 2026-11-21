use std::io;
use std::time::{Duration, SystemTime};

use openssl::ocsp::{OcspCertStatus, OcspFlag, OcspResponse, OcspResponseStatus};

use super::certificate::{CertificateIdentity, openssl_error};

pub(super) struct ValidatedResponse {
    pub(super) der: Vec<u8>,
    pub(super) next_update: Option<SystemTime>,
}

pub(super) fn validate_static(response: &[u8]) -> io::Result<()> {
    let response = OcspResponse::from_der(response).map_err(openssl_error)?;
    if response.status() != OcspResponseStatus::SUCCESSFUL {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("OCSP response status {:?}", response.status()),
        ));
    }
    Ok(())
}

pub(super) fn validate(
    identity: &CertificateIdentity,
    response: &[u8],
) -> io::Result<ValidatedResponse> {
    let response = OcspResponse::from_der(response).map_err(openssl_error)?;
    if response.status() != OcspResponseStatus::SUCCESSFUL {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("OCSP response status {:?}", response.status()),
        ));
    }
    let basic = response.basic().map_err(openssl_error)?;
    let flags = if identity.verify {
        OcspFlag::TRUST_OTHER
    } else {
        OcspFlag::NO_VERIFY
    };
    basic
        .verify(&identity.chain, &identity.store, flags)
        .map_err(openssl_error)?;
    let certificate_id = identity.certificate_id()?;
    let status = basic
        .find_status(&certificate_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "OCSP status not found"))?;
    if status.status != OcspCertStatus::GOOD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("OCSP certificate status {:?}", status.status),
        ));
    }
    status.check_validity(300, None).map_err(openssl_error)?;
    let next_update = status.next_update().map(parse_time).transpose()?;
    Ok(ValidatedResponse {
        der: response.to_der().map_err(openssl_error)?,
        next_update,
    })
}

fn parse_time(time: &openssl::asn1::Asn1GeneralizedTimeRef) -> io::Result<SystemTime> {
    let value = time.to_string();
    let fields = value.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 || fields[4] != "GMT" {
        return Err(invalid_time(&value));
    }
    let month = match fields[0] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return Err(invalid_time(&value)),
    };
    let day = fields[1].parse::<u32>().map_err(|_| invalid_time(&value))?;
    let mut clock = fields[2].split(':');
    let hour = clock.next().and_then(|value| value.parse::<u32>().ok());
    let minute = clock.next().and_then(|value| value.parse::<u32>().ok());
    let second = clock.next().and_then(|value| value.parse::<u32>().ok());
    let year = fields[3].parse::<i64>().map_err(|_| invalid_time(&value))?;
    let (Some(hour), Some(minute), Some(second)) = (hour, minute, second) else {
        return Err(invalid_time(&value));
    };
    if clock.next().is_some()
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || year < 1970
    {
        return Err(invalid_time(&value));
    }
    let seconds = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second);
    if seconds < 0 {
        return Err(invalid_time(&value));
    }
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds as u64))
        .ok_or_else(|| invalid_time(&value))
}

fn days_from_civil(mut year: i64, month: u32, day: u32) -> i64 {
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day = i64::from(day);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn invalid_time(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid OCSP nextUpdate \"{value}\""),
    )
}

pub(super) fn refresh_delay(now: SystemTime, next_update: Option<SystemTime>) -> Duration {
    let latest = next_update
        .and_then(|next_update| next_update.duration_since(now).ok())
        .unwrap_or(Duration::from_secs(24 * 60 * 60));
    latest
        .saturating_sub(Duration::from_secs(300))
        .clamp(Duration::from_secs(300), Duration::from_secs(3600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_window_matches_nginx() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(
            refresh_delay(now, Some(now + Duration::from_secs(200))),
            Duration::from_secs(300)
        );
        assert_eq!(
            refresh_delay(now, Some(now + Duration::from_secs(1200))),
            Duration::from_secs(900)
        );
        assert_eq!(
            refresh_delay(now, Some(now + Duration::from_secs(10_000))),
            Duration::from_secs(3600)
        );
        assert_eq!(refresh_delay(now, None), Duration::from_secs(3600));
    }
}
