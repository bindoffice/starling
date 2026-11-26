use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scheme {
    Http,
    Https,
}

impl Scheme {
    fn default_port(self) -> u16 {
        match self {
            Self::Http => 80,
            Self::Https => 443,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ResponderUrl {
    pub(super) scheme: Scheme,
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) path: String,
    pub(super) query: Option<String>,
}

impl ResponderUrl {
    pub(super) fn parse(value: &str) -> io::Result<Self> {
        let lower = value.to_ascii_lowercase();
        let (scheme, rest) = if lower.starts_with("http://") {
            (Scheme::Http, &value[7..])
        } else if lower.starts_with("https://") {
            (Scheme::Https, &value[8..])
        } else {
            return Err(invalid_url(value));
        };
        if rest.contains('#') {
            return Err(invalid_url(value));
        }
        let (authority, path_and_query) = rest
            .split_once('/')
            .map_or((rest, "/"), |(authority, path)| (authority, path));
        if authority.is_empty() || authority.contains('@') {
            return Err(invalid_url(value));
        }

        let (host, port) = parse_authority(authority, scheme.default_port(), value)?;
        let (path, query) = path_and_query
            .split_once('?')
            .map_or((path_and_query, None), |(path, query)| {
                (path, Some(query.to_string()))
            });
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        Ok(Self {
            scheme,
            host,
            port,
            path,
            query,
        })
    }

    pub(super) fn redirect(&self, location: &str) -> io::Result<Self> {
        let location = location.trim();
        if location.starts_with("http://") || location.starts_with("https://") {
            return Self::parse(location);
        }
        if location.starts_with("//") || location.contains('#') {
            return Err(invalid_redirect(location));
        }
        if location.starts_with('/') {
            let mut redirected = Self::parse(&format!(
                "{}://{}:{}{}",
                self.scheme_name(),
                format_host(&self.host),
                self.port,
                location
            ))?;
            redirected.scheme = self.scheme;
            return Ok(redirected);
        }

        let base_end = self.path.rfind('/').map_or(0, |index| index + 1);
        let mut redirected = Self::parse(&format!(
            "{}://{}:{}{}{}",
            self.scheme_name(),
            format_host(&self.host),
            self.port,
            &self.path[..base_end],
            location
        ))?;
        redirected.scheme = self.scheme;
        Ok(redirected)
    }

    pub(super) fn request_path(&self, encoded_request: &str) -> String {
        let mut path = self.path.clone();
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(encoded_request);
        if let Some(query) = &self.query {
            path.push('?');
            path.push_str(query);
        }
        path
    }

    pub(super) fn host_header(&self) -> String {
        if self.port == self.scheme.default_port() {
            format_host(&self.host)
        } else {
            format!("{}:{}", format_host(&self.host), self.port)
        }
    }

    pub(super) fn cache_key(&self) -> String {
        format!(
            "{}://{}:{}{}{}",
            self.scheme_name(),
            format_host(&self.host),
            self.port,
            self.path,
            self.query
                .as_ref()
                .map(|query| format!("?{query}"))
                .unwrap_or_default()
        )
    }

    fn scheme_name(&self) -> &'static str {
        match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        }
    }
}

fn parse_authority(
    authority: &str,
    default_port: u16,
    original: &str,
) -> io::Result<(String, u16)> {
    let (host, port) = if let Some(authority) = authority.strip_prefix('[') {
        let Some((host, suffix)) = authority.split_once(']') else {
            return Err(invalid_url(original));
        };
        let port = match suffix {
            "" => default_port,
            suffix => suffix
                .strip_prefix(':')
                .ok_or_else(|| invalid_url(original))?
                .parse()
                .map_err(|_| invalid_url(original))?,
        };
        (host.to_string(), port)
    } else if authority.matches(':').count() == 1 {
        let (host, port) = authority.rsplit_once(':').expect("one colon");
        (
            host.to_string(),
            port.parse().map_err(|_| invalid_url(original))?,
        )
    } else {
        (authority.to_string(), default_port)
    };
    if host.is_empty() {
        return Err(invalid_url(original));
    }
    Ok((host, port))
}

fn format_host(host: &str) -> String {
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

fn invalid_url(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid OCSP responder URL \"{value}\""),
    )
}

fn invalid_redirect(value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid OCSP redirect location \"{value}\""),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_https_ipv4_ipv6_and_paths() {
        let responder = ResponderUrl::parse("http://127.0.0.1:8080/check").unwrap();
        assert_eq!(responder.scheme, Scheme::Http);
        assert_eq!(responder.host, "127.0.0.1");
        assert_eq!(responder.port, 8080);
        assert_eq!(responder.path, "/check");

        let responder = ResponderUrl::parse("HTTPS://[::1]/ocsp?source=ca").unwrap();
        assert_eq!(responder.scheme, Scheme::Https);
        assert_eq!(responder.host, "::1");
        assert_eq!(responder.port, 443);
        assert_eq!(responder.path, "/ocsp");
        assert_eq!(responder.host_header(), "[::1]");
        assert_eq!(responder.request_path("REQUEST"), "/ocsp/REQUEST?source=ca");
    }

    #[test]
    fn resolves_absolute_and_relative_redirects() {
        let responder = ResponderUrl::parse("https://example.test:8443/base/ocsp").unwrap();
        let absolute = responder.redirect("http://127.0.0.1:8080/new").unwrap();
        assert_eq!(absolute.scheme, Scheme::Http);
        assert_eq!(absolute.host, "127.0.0.1");

        let relative = responder.redirect("next").unwrap();
        assert_eq!(relative.scheme, Scheme::Https);
        assert_eq!(relative.path, "/base/next");

        let root = responder.redirect("/new").unwrap();
        assert_eq!(root.path, "/new");
    }
}
