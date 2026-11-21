#[derive(Debug, Clone)]
pub struct ExpiresRule {
    pub value: ExpiresValue,
}

#[derive(Debug, Clone)]
pub enum ExpiresValue {
    Off,
    Epoch,
    Max,
    Access(ExpiresOffset),
    Modified(ExpiresOffset),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IfModifiedSince {
    Off,
    #[default]
    Exact,
    Before,
}

#[derive(Debug, Clone)]
pub enum ExpiresOffset {
    Fixed(i64),
    Daily(u32),
    Variable(String),
}

impl ExpiresValue {
    pub fn parse_literal(value: &str, modified: bool) -> Option<Self> {
        if !modified {
            match value {
                "off" => return Some(Self::Off),
                "epoch" => return Some(Self::Epoch),
                "max" => return Some(Self::Max),
                _ => {}
            }
        }

        let (daily, value) = match value.strip_prefix('@') {
            Some(value) => (true, value),
            None => (false, value),
        };
        if daily && modified {
            return None;
        }
        let (negative, value) = if daily {
            (false, value)
        } else if let Some(value) = value.strip_prefix('-') {
            (true, value)
        } else {
            (false, value.strip_prefix('+').unwrap_or(value))
        };
        let seconds = parse_seconds(value)?;
        if daily && seconds > 24 * 60 * 60 {
            return None;
        }
        let seconds = if negative {
            i64::try_from(seconds).ok()?.checked_neg()?
        } else {
            i64::try_from(seconds).ok()?
        };
        Some(if modified {
            Self::Modified(ExpiresOffset::Fixed(seconds))
        } else if daily {
            Self::Access(ExpiresOffset::Daily(seconds as u32))
        } else {
            Self::Access(ExpiresOffset::Fixed(seconds))
        })
    }
}

fn parse_seconds(value: &str) -> Option<u64> {
    const UNITS: [(u8, u8, u64); 7] = [
        (b'y', 0, 365 * 24 * 60 * 60),
        (b'M', 1, 30 * 24 * 60 * 60),
        (b'w', 2, 7 * 24 * 60 * 60),
        (b'd', 3, 24 * 60 * 60),
        (b'h', 4, 60 * 60),
        (b'm', 5, 60),
        (b's', 6, 1),
    ];

    if value.is_empty() {
        return None;
    }
    let bytes = value.as_bytes();
    let mut position = 0;
    let mut total = 0_u64;
    let mut last_rank = None;
    let mut saw_number = false;
    while position < bytes.len() {
        let start = position;
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
        if position == start {
            return None;
        }
        saw_number = true;
        let number = value[start..position].parse::<u64>().ok()?;
        let (scale, rank) = if position == bytes.len() {
            (1, None)
        } else {
            let unit = bytes[position];
            position += 1;
            let (_, rank, scale) = UNITS.iter().find(|(name, _, _)| *name == unit)?;
            if last_rank.is_some_and(|last| *rank <= last) {
                return None;
            }
            (*scale, Some(*rank))
        };
        total = total.checked_add(number.checked_mul(scale)?)?;
        if rank.is_none() {
            break;
        }
        last_rank = rank;
        while position < bytes.len() && bytes[position] == b' ' {
            position += 1;
        }
    }
    saw_number.then_some(total)
}
