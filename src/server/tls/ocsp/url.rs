use std::io;

#[derive(Clone, Debug)]
pub(super) struct ResponderUrl {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) path: String,
}

impl ResponderUrl {
    pub(super) fn parse(value: &str) -> io::Result<Self> {
        let Some(rest) = value
            .get(..7)
            .filter(|prefix| prefix.eq_ignore_ascii_case("http://"))
        else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid OCSP responder URL \"{value}\""),
            ));
        };
        let rest = value
            .get(rest.len()..)
            .expect("HTTP URL prefix has a fixed length");
        let (authority, path) = rest
            .split_once('/')
            .map_or((rest, "/".to_string()), |(authority, path)| {
                (authority, format!("/{path}"))
            });
        if authority.is_empty() || authority.contains('@') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid OCSP responder URL \"{value}\""),
            ));
        }

        let (host, port) = if let Some(authority) = authority.strip_prefix('[') {
            let Some((host, suffix)) = authority.split_once(']') else {
                return Err(invalid_url(value));
            };
            let port = match suffix {
                "" => 80,
                suffix => suffix
                    .strip_prefix(':')
                    .ok_or_else(|| invalid_url(value))?
                    .parse()
                    .map_err(|_| invalid_url(value))?,
            };
            (host.to_string(), port)
        } else if authority.matches(':').count() == 1 {
            let (host, port) = authority.rsplit_once(':').expect("one colon");
            (
                host.to_string(),
                port.parse().map_err(|_| invalid_url(value))?,
            )
        } else {
            (authority.to_string(), 80)
        };
        if host.is_empty() {
            return Err(invalid_url(value));
        }
        Ok(Self { host, port, path })
    }

    pub(super) fn host_header(&self) -> String {
        if self.host.contains(':') && self.port == 80 {
            format!("[{}]", self.host)
        } else if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

fn invalid_url(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid OCSP responder URL \"{value}\""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ipv4_ipv6_and_paths() {
        let responder = ResponderUrl::parse("http://127.0.0.1:8080/check").unwrap();
        assert_eq!(responder.host, "127.0.0.1");
        assert_eq!(responder.port, 8080);
        assert_eq!(responder.path, "/check");

        let responder = ResponderUrl::parse("HTTP://[::1]/ocsp").unwrap();
        assert_eq!(responder.host, "::1");
        assert_eq!(responder.port, 80);
        assert_eq!(responder.path, "/ocsp");
        assert_eq!(responder.host_header(), "[::1]");
    }
}
