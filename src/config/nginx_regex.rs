use std::fmt;

pub type Captures<'t> = fancy_regex::Captures<'t>;
pub type Error = Box<fancy_regex::Error>;

#[derive(Clone)]
pub struct Regex(fancy_regex::Regex);

impl Regex {
    pub fn new(pattern: &str) -> Result<Self, Error> {
        fancy_regex::Regex::new(pattern).map(Self).map_err(Box::new)
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
        Self(fancy_regex::RegexBuilder::new(pattern))
    }

    pub fn case_insensitive(&mut self, yes: bool) -> &mut Self {
        self.0.case_insensitive(yes);
        self
    }

    pub fn build(&self) -> Result<Regex, Error> {
        self.0.build().map(Regex).map_err(Box::new)
    }
}
