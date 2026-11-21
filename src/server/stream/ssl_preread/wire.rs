pub(super) fn ssl_version_name(major: u8, minor: u8) -> &'static str {
    match (major, minor) {
        (0, 2) => "SSLv2",
        (3, 0) => "SSLv3",
        (3, 1) => "TLSv1",
        (3, 2) => "TLSv1.1",
        (3, 3) => "TLSv1.2",
        (3, 4) => "TLSv1.3",
        _ => "",
    }
}

pub(super) fn take_u8(data: &[u8], position: &mut usize) -> Option<u8> {
    Some(take(data, position, 1)?[0])
}

pub(super) fn take_u16(data: &[u8], position: &mut usize) -> Option<u16> {
    let bytes = take(data, position, 2)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

pub(super) fn take<'a>(data: &'a [u8], position: &mut usize, length: usize) -> Option<&'a [u8]> {
    let end = position.checked_add(length)?;
    let value = data.get(*position..end)?;
    *position = end;
    Some(value)
}
