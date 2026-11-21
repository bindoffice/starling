use super::*;

pub(super) struct Destination {
    pub uri: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Depth {
    Zero,
    One,
    Infinity,
}

pub(super) fn source_path(
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    uri: &str,
) -> PathBuf {
    map_request_path(context, location, uri)
}

pub(super) fn parse_destination(
    request: &Request,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
) -> Result<Destination, u16> {
    let value = request.header("destination").ok_or(400_u16)?;
    let raw_path = destination_path(value, request).ok_or(400_u16)?;
    let raw_path = raw_path.split(['?', '#']).next().unwrap_or(raw_path);
    let uri = normalize_uri(raw_path).ok_or(400_u16)?;
    validate_alias_prefix(&uri, location, context)?;
    let path = source_path(context, location, &uri);
    Ok(Destination { uri, path })
}

pub(super) fn depth(request: &Request, default: Depth) -> Result<Depth, u16> {
    let Some(value) = request.header("depth") else {
        return Ok(default);
    };
    match value {
        "0" => Ok(Depth::Zero),
        "1" => Ok(Depth::One),
        "infinity" => Ok(Depth::Infinity),
        _ => Err(400),
    }
}

pub(super) fn request_has_body(request: &Request) -> bool {
    !request.body.is_empty()
        || request
            .header("content-length")
            .is_some_and(|value| value != "0")
        || request.header("transfer-encoding").is_some_and(|value| {
            value
                .split(',')
                .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
        })
}

pub(super) fn location_header(uri: &str) -> String {
    const ESCAPE: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'\'')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}')
        .add(b'|')
        .add(b'\\')
        .add(b'^')
        .add(b'[')
        .add(b']');
    utf8_percent_encode(uri, ESCAPE).to_string()
}

pub(super) fn same_or_descendant(source: &Path, destination: &Path) -> bool {
    source == destination || destination.starts_with(source)
}

fn destination_path<'a>(value: &'a str, request: &Request) -> Option<&'a str> {
    if value.starts_with('/') {
        return Some(value);
    }
    let (_, rest) = value.split_once("://")?;
    let (authority, _) = rest.split_once('/')?;
    let host = request.host_without_port();
    if !authority.eq_ignore_ascii_case(host)
        && !authority.get(host.len()..).is_some_and(|suffix| {
            authority[..host.len()].eq_ignore_ascii_case(host) && suffix.starts_with(':')
        })
    {
        return None;
    }
    Some(&rest[authority.len()..])
}

fn validate_alias_prefix(
    uri: &str,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
) -> Result<(), u16> {
    if context.alias.is_none() {
        return Ok(());
    }
    let Some(location) = location else {
        return Err(400);
    };
    let prefix = match &location.matcher {
        LocationMatcher::Exact(path)
        | LocationMatcher::Prefix(path)
        | LocationMatcher::PrefixNoRegex(path) => path,
        LocationMatcher::Regex(_) | LocationMatcher::Named(_) => return Err(400),
    };
    if uri.starts_with(prefix) {
        Ok(())
    } else {
        Err(400)
    }
}
