#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    fn len(self) -> u64 {
        self.end - self.start
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ParsedRanges {
    Ignore,
    Unsatisfiable,
    Satisfiable(Vec<ByteRange>),
}

pub(super) fn parse_ranges(spec: &[u8], length: u64, max_ranges: u64) -> ParsedRanges {
    let mut position = 0;
    let mut ranges = Vec::new();
    let mut size = 0_u64;

    loop {
        skip_spaces(spec, &mut position);
        let suffix = spec.get(position) == Some(&b'-');
        let start = if suffix {
            position += 1;
            None
        } else {
            let Ok(Some(start)) = parse_number(spec, &mut position) else {
                return ParsedRanges::Unsatisfiable;
            };
            skip_spaces(spec, &mut position);
            if spec.get(position) != Some(&b'-') {
                return ParsedRanges::Unsatisfiable;
            }
            position += 1;
            skip_spaces(spec, &mut position);
            Some(start)
        };

        let end = match parse_number(spec, &mut position) {
            Ok(end) => end,
            Err(()) => return ParsedRanges::Unsatisfiable,
        };
        if suffix && end.is_none() {
            return ParsedRanges::Unsatisfiable;
        }
        skip_spaces(spec, &mut position);
        if position < spec.len() && spec[position] != b',' {
            return ParsedRanges::Unsatisfiable;
        }

        let range = match (start, end) {
            (Some(start), Some(end)) => {
                let end = if end >= length {
                    length
                } else {
                    let Some(end) = end.checked_add(1) else {
                        return ParsedRanges::Unsatisfiable;
                    };
                    end
                };
                ByteRange { start, end }
            }
            (Some(start), None) => ByteRange { start, end: length },
            (None, Some(suffix)) => ByteRange {
                start: length.saturating_sub(suffix),
                end: length,
            },
            (None, None) => return ParsedRanges::Unsatisfiable,
        };

        if range.start < range.end {
            if ranges.len() as u64 >= max_ranges {
                return ParsedRanges::Ignore;
            }
            let Some(next_size) = size.checked_add(range.len()) else {
                return ParsedRanges::Unsatisfiable;
            };
            size = next_size;
            ranges.push(range);
        } else if range.start == 0 {
            return ParsedRanges::Ignore;
        }

        if spec.get(position) != Some(&b',') {
            break;
        }
        position += 1;
    }

    if ranges.is_empty() {
        return ParsedRanges::Unsatisfiable;
    }
    if ranges.len() == 1 {
        return ParsedRanges::Satisfiable(ranges);
    }
    if size > length {
        return ParsedRanges::Ignore;
    }
    if max_ranges == i32::MAX as u64 {
        let with_overhead = size.saturating_add((ranges.len() as u64).saturating_mul(256));
        if with_overhead > length.saturating_add(4096) {
            return ParsedRanges::Ignore;
        }
    }
    ParsedRanges::Satisfiable(ranges)
}

fn skip_spaces(value: &[u8], position: &mut usize) {
    while value.get(*position) == Some(&b' ') {
        *position += 1;
    }
}

fn parse_number(value: &[u8], position: &mut usize) -> Result<Option<u64>, ()> {
    let start = *position;
    let mut number = 0_u64;
    while let Some(byte) = value.get(*position)
        && byte.is_ascii_digit()
    {
        number = number
            .checked_mul(10)
            .and_then(|number| number.checked_add(u64::from(byte - b'0')))
            .ok_or(())?;
        if number > i64::MAX as u64 {
            return Err(());
        }
        *position += 1;
    }
    Ok((*position > start).then_some(number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_ranges_in_order() {
        assert_eq!(
            parse_ranges(b"0-1, 4-6,-2", 10, i32::MAX as u64),
            ParsedRanges::Satisfiable(vec![
                ByteRange { start: 0, end: 2 },
                ByteRange { start: 4, end: 7 },
                ByteRange { start: 8, end: 10 },
            ])
        );
    }

    #[test]
    fn applies_nginx_range_limits_and_overhead_heuristic() {
        assert_eq!(parse_ranges(b"0-1,3-4", 10, 1), ParsedRanges::Ignore);
        assert_eq!(
            parse_ranges(b"0-1,3-4", 10, i32::MAX as u64),
            ParsedRanges::Satisfiable(vec![
                ByteRange { start: 0, end: 2 },
                ByteRange { start: 3, end: 5 },
            ])
        );
        assert_eq!(
            parse_ranges(b"0-1,3-4", 10, 2),
            ParsedRanges::Satisfiable(vec![
                ByteRange { start: 0, end: 2 },
                ByteRange { start: 3, end: 5 },
            ])
        );
        let many = vec!["0-0"; 17].join(",");
        assert_eq!(
            parse_ranges(many.as_bytes(), 100, i32::MAX as u64),
            ParsedRanges::Ignore
        );
    }

    #[test]
    fn matches_nginx_edge_cases() {
        assert_eq!(parse_ranges(b"0-0", 0, 10), ParsedRanges::Ignore);
        assert_eq!(parse_ranges(b"99-100", 10, 10), ParsedRanges::Unsatisfiable);
        assert_eq!(
            parse_ranges(b"0-1,broken", 10, 10),
            ParsedRanges::Unsatisfiable
        );
        assert_eq!(
            parse_ranges(b"0-1,20-30", 10, 10),
            ParsedRanges::Satisfiable(vec![ByteRange { start: 0, end: 2 }])
        );
    }
}
