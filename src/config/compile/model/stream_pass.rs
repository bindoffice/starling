use super::*;

#[derive(Debug, Clone)]
pub struct StreamPass {
    pub target: String,
    pub address: Option<ListenAddress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamHandler {
    Proxy,
    Return,
    Pass,
}

pub(crate) fn parse_stream_pass_target(
    value: &str,
    prefix: &Path,
) -> Result<ListenAddress, String> {
    if let Some(path) = value.strip_prefix("unix:") {
        if path.is_empty() {
            return Err("no path in the unix domain socket".into());
        }
        let path = Path::new(path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            prefix.join(path)
        };
        if path.to_string_lossy().len() >= 108 {
            return Err("too long path in the unix domain socket".into());
        }
        return Ok(ListenAddress::Unix(path));
    }

    let (host, port) = split_pass_target(value)?;
    let port = port
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "invalid port".to_string())?;
    let address = host
        .parse::<IpAddr>()
        .map_err(|_| "has no addresses".to_string())?;
    Ok(ListenAddress::Inet(SocketAddr::new(address, port)))
}

fn split_pass_target(value: &str) -> Result<(&str, &str), String> {
    if let Some(rest) = value.strip_prefix('[') {
        return rest
            .split_once("]:")
            .ok_or_else(|| "has no port".to_string());
    }
    value
        .rsplit_once(':')
        .ok_or_else(|| "has no port".to_string())
}
