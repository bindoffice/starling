#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IntegerPurpose {
    FieldIndex,
    LiteralNameIndex,
    TableSizeUpdate,
    NameLength,
    ValueLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StringPart {
    Name,
    Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingInteger {
    value: usize,
    shift: u32,
    octets: u8,
    purpose: IntegerPurpose,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Representation,
    Integer(PendingInteger),
    StringLength(StringPart),
    String { remaining: usize, part: StringPart },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HeaderBlockError {
    FrameSize,
    Compression,
    FieldLimit,
}

pub(super) struct HeaderBlockValidator {
    field_limit: usize,
    state: State,
}

impl HeaderBlockValidator {
    pub(super) fn new(field_limit: usize) -> Self {
        Self {
            field_limit,
            state: State::Representation,
        }
    }

    pub(super) fn reset(&mut self) {
        self.state = State::Representation;
    }

    pub(super) fn validate(&mut self, mut bytes: &[u8]) -> Result<(), HeaderBlockError> {
        if self.field_limit == 0 {
            return Ok(());
        }

        while !bytes.is_empty() {
            if let State::String { remaining, part } = &mut self.state {
                let consumed = (*remaining).min(bytes.len());
                *remaining -= consumed;
                bytes = &bytes[consumed..];
                if *remaining != 0 {
                    return Ok(());
                }
                self.state = match part {
                    StringPart::Name => State::StringLength(StringPart::Value),
                    StringPart::Value => State::Representation,
                };
                continue;
            }

            let byte = bytes[0];
            bytes = &bytes[1..];
            let state = std::mem::replace(&mut self.state, State::Representation);
            self.state = match state {
                State::Representation => self.start_representation(byte)?,
                State::Integer(pending) => self.continue_integer(pending, byte)?,
                State::StringLength(part) => self.start_string(byte, part)?,
                State::String { .. } => unreachable!("string state is handled above"),
            };
        }
        Ok(())
    }

    pub(super) fn finish(&self) -> Result<(), HeaderBlockError> {
        if matches!(self.state, State::Representation) {
            Ok(())
        } else {
            Err(HeaderBlockError::FrameSize)
        }
    }

    fn start_representation(&self, byte: u8) -> Result<State, HeaderBlockError> {
        match byte {
            0x80..=0xff => self.start_integer(byte, 7, IntegerPurpose::FieldIndex),
            0x40..=0x7f => self.start_integer(byte, 6, IntegerPurpose::LiteralNameIndex),
            0x20..=0x3f => self.start_integer(byte, 5, IntegerPurpose::TableSizeUpdate),
            _ => self.start_integer(byte, 4, IntegerPurpose::LiteralNameIndex),
        }
    }

    fn start_string(&self, byte: u8, part: StringPart) -> Result<State, HeaderBlockError> {
        let purpose = match part {
            StringPart::Name => IntegerPurpose::NameLength,
            StringPart::Value => IntegerPurpose::ValueLength,
        };
        self.start_integer(byte, 7, purpose)
    }

    fn start_integer(
        &self,
        byte: u8,
        prefix_bits: u32,
        purpose: IntegerPurpose,
    ) -> Result<State, HeaderBlockError> {
        let prefix_max = (1_usize << prefix_bits) - 1;
        let value = usize::from(byte) & prefix_max;
        if value < prefix_max {
            self.complete_integer(value, purpose)
        } else {
            Ok(State::Integer(PendingInteger {
                value,
                shift: 0,
                octets: 1,
                purpose,
            }))
        }
    }

    fn continue_integer(
        &self,
        mut pending: PendingInteger,
        byte: u8,
    ) -> Result<State, HeaderBlockError> {
        let contribution = usize::from(byte & 0x7f)
            .checked_shl(pending.shift)
            .ok_or(HeaderBlockError::Compression)?;
        pending.value = pending
            .value
            .checked_add(contribution)
            .ok_or(HeaderBlockError::Compression)?;
        pending.octets += 1;

        if byte & 0x80 == 0 {
            self.complete_integer(pending.value, pending.purpose)
        } else {
            if pending.octets >= 4 {
                return Err(HeaderBlockError::Compression);
            }
            pending.shift = pending.shift.saturating_add(7);
            Ok(State::Integer(pending))
        }
    }

    fn complete_integer(
        &self,
        value: usize,
        purpose: IntegerPurpose,
    ) -> Result<State, HeaderBlockError> {
        match purpose {
            IntegerPurpose::FieldIndex | IntegerPurpose::TableSizeUpdate => {
                Ok(State::Representation)
            }
            IntegerPurpose::LiteralNameIndex => Ok(State::StringLength(if value == 0 {
                StringPart::Name
            } else {
                StringPart::Value
            })),
            IntegerPurpose::NameLength | IntegerPurpose::ValueLength => {
                if value > self.field_limit {
                    return Err(HeaderBlockError::FieldLimit);
                }
                let part = if purpose == IntegerPurpose::NameLength {
                    StringPart::Name
                } else {
                    StringPart::Value
                };
                Ok(if value == 0 {
                    self.string_complete(part)
                } else {
                    State::String {
                        remaining: value,
                        part,
                    }
                })
            }
        }
    }

    fn string_complete(&self, part: StringPart) -> State {
        match part {
            StringPart::Name => State::StringLength(StringPart::Value),
            StringPart::Value => State::Representation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_literal_strings_at_the_encoded_limit() {
        let mut validator = HeaderBlockValidator::new(4);
        let mut block = vec![0x00, 0x04];
        block.extend_from_slice(b"name");
        block.extend_from_slice(&[0x04]);
        block.extend_from_slice(b"value");
        assert_eq!(validator.validate(&block), Ok(()));
    }

    #[test]
    fn rejects_literal_strings_over_the_encoded_limit() {
        let mut validator = HeaderBlockValidator::new(4);
        let block = [0x00, 0x05, b'n', b'a', b'm', b'e', b'x'];
        assert_eq!(
            validator.validate(&block),
            Err(HeaderBlockError::FieldLimit)
        );
    }

    #[test]
    fn checks_huffman_encoded_length_without_decoding() {
        let mut validator = HeaderBlockValidator::new(3);
        let block = [0x00, 0x04, b'n', b'a', b'm', b'e'];
        assert_eq!(
            validator.validate(&block),
            Err(HeaderBlockError::FieldLimit)
        );

        let mut validator = HeaderBlockValidator::new(4);
        let block = [0x00, 0x84, 0xff, 0xff, 0xff, 0xff];
        assert_eq!(validator.validate(&block), Ok(()));
    }

    #[test]
    fn validates_strings_split_across_frames() {
        let mut validator = HeaderBlockValidator::new(4);
        assert_eq!(validator.validate(&[0x00, 0x02, b'n']), Ok(()));
        assert_eq!(validator.validate(&[b'a', 0x04, b'v', b'a']), Ok(()));
        assert_eq!(validator.validate(b"lue"), Ok(()));
    }

    #[test]
    fn validates_multi_byte_integer_lengths() {
        let mut validator = HeaderBlockValidator::new(130);
        let mut block = vec![0x00, 0x7f, 0x03];
        block.extend(std::iter::repeat_n(b'n', 130));
        block.extend_from_slice(&[0x00]);
        assert_eq!(validator.validate(&block), Ok(()));

        let mut validator = HeaderBlockValidator::new(129);
        assert_eq!(
            validator.validate(&block),
            Err(HeaderBlockError::FieldLimit)
        );
    }

    #[test]
    fn ignores_indexed_fields_and_table_size_updates() {
        let mut validator = HeaderBlockValidator::new(1);
        assert_eq!(validator.validate(&[0x82, 0x3f, 0x01]), Ok(()));
    }

    #[test]
    fn applies_the_limit_to_literal_names_with_indexed_values() {
        let mut validator = HeaderBlockValidator::new(3);
        let block = [0x00, 0x04, b'x', b'y', b'z', b'w', 0x00];
        assert_eq!(
            validator.validate(&block),
            Err(HeaderBlockError::FieldLimit)
        );
    }

    #[test]
    fn rejects_overlong_integers_as_compression_errors() {
        let mut validator = HeaderBlockValidator::new(1024);
        assert_eq!(
            validator.validate(&[0xff, 0xff, 0xff, 0xff]),
            Err(HeaderBlockError::Compression)
        );
    }

    #[test]
    fn detects_truncated_integers_at_the_end_of_a_header_block() {
        let mut validator = HeaderBlockValidator::new(1024);
        assert_eq!(validator.validate(&[0xff]), Ok(()));
        assert_eq!(validator.finish(), Err(HeaderBlockError::FrameSize));
    }

    #[test]
    fn accepts_huffman_literals_from_the_nginx_test_suite() {
        let mut validator = HeaderBlockValidator::new(1024);
        let block = [
            0x42, 0x83, 0xc5, 0x83, 0x7f, 0x46, 0x83, 0x9d, 0x29, 0xaf, 0x44, 0x81, 0x63, 0x41,
            0x87, 0xa0, 0xe4, 0x1d, 0x13, 0x9d, 0x09, 0xff,
        ];
        assert_eq!(validator.validate(&block), Ok(()));
    }
}
