use std::fmt;

pub type Captures<'t> = fancy_regex::Captures<'t>;
pub type Error = Box<fancy_regex::Error>;

#[derive(Clone)]
pub struct Regex(fancy_regex::Regex);

impl Regex {
    pub fn new(pattern: &str) -> Result<Self, Error> {
        fancy_regex::Regex::new(&normalize_pcre_quotes(pattern))
            .map(Self)
            .map_err(Box::new)
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(text).unwrap_or(false)
    }

    pub fn captures<'t>(&self, text: &'t str) -> Option<Captures<'t>> {
        self.0.captures(text).ok().flatten()
    }

    pub fn capture_names(&self) -> impl Iterator<Item = Option<&str>> + '_ {
        self.0.capture_names()
    }

    pub fn captures_len(&self) -> usize {
        self.0.captures_len()
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for Regex {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Regex")
            .field(&self.0.as_str())
            .finish()
    }
}

pub struct RegexBuilder(fancy_regex::RegexBuilder);

impl RegexBuilder {
    pub fn new(pattern: &str) -> Self {
        Self(fancy_regex::RegexBuilder::new(&normalize_pcre_quotes(
            pattern,
        )))
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.0.case_insensitive(yes);
        self
    }

    pub fn build(&self) -> Result<Regex, Error> {
        self.0.build().map(Regex).map_err(Box::new)
    }
}

fn normalize_pcre_quotes(pattern: &str) -> std::borrow::Cow<'_, str> {
    let Some(first) = pattern.find("\\Q") else {
        return std::borrow::Cow::Borrowed(pattern);
    };
    let mut output = String::with_capacity(pattern.len());
    output.push_str(&pattern[..first]);
    let mut index = first;
    while index < pattern.len() {
        let rest = &pattern[index..];
        if let Some(literal) = rest.strip_prefix("\\Q") {
            if let Some(end) = literal.find("\\E") {
                output.push_str(&regex::escape(&literal[..end]));
                index += 2 + end + 2;
            } else {
                output.push_str(&regex::escape(literal));
                break;
            }
        } else {
            let character = rest.chars().next().expect("non-empty regex suffix");
            let mut encoded = [0; 4];
            output.push_str(character.encode_utf8(&mut encoded));
            index += character.len_utf8();
        }
    }
    std::borrow::Cow::Owned(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_pcre_quoted_literals_before_compilation() {
        let regex = Regex::new(r"^(?P<name>[a-z]+)\Q.example.com\E$").unwrap();
        assert!(regex.is_match("www.example.com"));
        assert!(!regex.is_match("wwwXexample.com"));
    }
}
