use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharsetConfig {
    pub charset: Option<String>,
    pub source_charset: Option<String>,
    pub override_charset: bool,
    pub types: Vec<String>,
}

impl Default for CharsetConfig {
    fn default() -> Self {
        Self {
            charset: None,
            source_charset: None,
            override_charset: false,
            types: vec![
                "text/html".into(),
                "text/xml".into(),
                "text/plain".into(),
                "text/vnd.wap.wml".into(),
                "application/javascript".into(),
                "application/rss+xml".into(),
            ],
        }
    }
}

#[derive(Debug, Clone)]
pub struct CharsetMap {
    pub source: String,
    pub destination: String,
    table: CharsetMapTable,
}

#[derive(Debug, Clone)]
enum CharsetMapTable {
    SingleByte {
        forward: Box<[u8; 256]>,
        reverse: Box<[u8; 256]>,
    },
    Utf8 {
        forward: Box<[Vec<u8>; 256]>,
        reverse: HashMap<u32, u8>,
    },
}

pub(crate) enum CharsetConversion<'a> {
    ToBytes(&'a [u8; 256]),
    ToUtf8(&'a [Vec<u8>; 256]),
    FromUtf8(&'a HashMap<u32, u8>),
}

impl CharsetMap {
    pub(crate) fn single_byte(source: String, destination: String) -> Self {
        let forward = Box::new(std::array::from_fn(|index| {
            if index < 128 { index as u8 } else { b'?' }
        }));
        let reverse = forward.clone();
        Self {
            source,
            destination,
            table: CharsetMapTable::SingleByte { forward, reverse },
        }
    }

    pub(crate) fn utf8_destination(source: String, destination: String) -> Self {
        let forward = Box::new(std::array::from_fn(|index| {
            if index < 128 {
                vec![index as u8]
            } else {
                vec![b'?']
            }
        }));
        let reverse = (0..128).map(|value| (value as u32, value as u8)).collect();
        Self {
            source,
            destination,
            table: CharsetMapTable::Utf8 { forward, reverse },
        }
    }

    pub(crate) fn set_single_byte(
        &mut self,
        source: u8,
        destination: u8,
    ) -> Result<(), &'static str> {
        let CharsetMapTable::SingleByte { forward, reverse } = &mut self.table else {
            return Err("invalid single-byte mapping");
        };
        forward[source as usize] = destination;
        reverse[destination as usize] = source;
        Ok(())
    }

    pub(crate) fn set_utf8(
        &mut self,
        source: u8,
        encoded: &[u8],
        codepoint: u32,
    ) -> Result<(), &'static str> {
        let CharsetMapTable::Utf8 { forward, reverse } = &mut self.table else {
            return Err("invalid UTF-8 mapping");
        };
        forward[source as usize] = encoded.to_vec();
        reverse.insert(codepoint, source);
        Ok(())
    }

    pub fn connects(&self, source: &str, destination: &str) -> bool {
        (self.source.eq_ignore_ascii_case(source)
            && self.destination.eq_ignore_ascii_case(destination))
            || (self.source.eq_ignore_ascii_case(destination)
                && self.destination.eq_ignore_ascii_case(source))
    }

    pub(crate) fn conversion(
        &self,
        source: &str,
        destination: &str,
    ) -> Option<CharsetConversion<'_>> {
        let is_forward = self.source.eq_ignore_ascii_case(source)
            && self.destination.eq_ignore_ascii_case(destination);
        let reverse = self.source.eq_ignore_ascii_case(destination)
            && self.destination.eq_ignore_ascii_case(source);
        if !is_forward && !reverse {
            return None;
        }
        match &self.table {
            CharsetMapTable::SingleByte { forward, reverse } => {
                Some(CharsetConversion::ToBytes(if is_forward {
                    forward.as_ref()
                } else {
                    reverse.as_ref()
                }))
            }
            CharsetMapTable::Utf8 { forward, reverse } => {
                if is_forward {
                    Some(CharsetConversion::ToUtf8(forward.as_ref()))
                } else {
                    Some(CharsetConversion::FromUtf8(reverse))
                }
            }
        }
    }

    pub(crate) fn includes(&self, name: &str) -> bool {
        self.source.eq_ignore_ascii_case(name) || self.destination.eq_ignore_ascii_case(name)
    }
}
