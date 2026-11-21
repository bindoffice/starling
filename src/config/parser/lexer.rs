use std::path::Path;

use crate::error::{ConfigError, ConfigResult, SourceLocation};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TokenKind {
    Word,
    Semicolon,
    OpenBrace,
    CloseBrace,
}

#[derive(Debug, Clone)]
pub(super) struct Token {
    pub(super) kind: TokenKind,
    pub(super) value: String,
    pub(super) line: usize,
    pub(super) column: usize,
}

pub(super) fn lex(source: &str, file: &Path) -> ConfigResult<Vec<Token>> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut line = 1;
    let mut column = 1;

    while index < bytes.len() {
        match bytes[index] {
            b' ' | b'\t' | b'\r' => {
                index += 1;
                column += 1;
            }
            b'\n' => {
                index += 1;
                line += 1;
                column = 1;
            }
            b'#' => {
                while index < bytes.len() && bytes[index] != b'\n' {
                    index += 1;
                    column += 1;
                }
            }
            b';' => {
                tokens.push(Token {
                    kind: TokenKind::Semicolon,
                    value: ";".into(),
                    line,
                    column,
                });
                index += 1;
                column += 1;
            }
            b'{' => {
                tokens.push(Token {
                    kind: TokenKind::OpenBrace,
                    value: "{".into(),
                    line,
                    column,
                });
                index += 1;
                column += 1;
            }
            b'}' => {
                tokens.push(Token {
                    kind: TokenKind::CloseBrace,
                    value: "}".into(),
                    line,
                    column,
                });
                index += 1;
                column += 1;
            }
            b'"' | b'\'' => {
                let quote = bytes[index];
                let start_line = line;
                let start_column = column;
                index += 1;
                column += 1;
                let mut value = String::new();
                let mut closed = false;
                while index < bytes.len() {
                    let byte = bytes[index];
                    if byte == quote {
                        index += 1;
                        column += 1;
                        closed = true;
                        break;
                    }
                    if byte == b'\\' {
                        index += 1;
                        column += 1;
                        if index >= bytes.len() {
                            break;
                        }
                        let escaped = bytes[index];
                        match escaped {
                            b'n' => value.push('\n'),
                            b'r' => value.push('\r'),
                            b't' => value.push('\t'),
                            b'\\' => value.push('\\'),
                            b'"' => value.push('"'),
                            b'\'' => value.push('\''),
                            b'\n' => {
                                line += 1;
                                column = 0;
                                index += 1;
                                column += 1;
                                continue;
                            }
                            other => {
                                value.push('\\');
                                value.push(other as char);
                            }
                        }
                        index += 1;
                        column += 1;
                        continue;
                    }
                    if byte == b'\n' {
                        line += 1;
                        column = 0;
                    }
                    if !is_utf8_boundary(source, index) {
                        return Err(ConfigError::at(
                            SourceLocation::new(file, line, column),
                            "invalid UTF-8 in configuration",
                        ));
                    }
                    let character = source[index..].chars().next().unwrap();
                    value.push(character);
                    index += character.len_utf8();
                    column += 1;
                }
                if !closed {
                    return Err(ConfigError::at(
                        SourceLocation::new(file, start_line, start_column),
                        "unexpected end of file, expecting closing quote",
                    ));
                }
                tokens.push(Token {
                    kind: TokenKind::Word,
                    value,
                    line: start_line,
                    column: start_column,
                });
            }
            _ => {
                let start_line = line;
                let start_column = column;
                let mut value = String::new();
                while index < bytes.len() {
                    let byte = bytes[index];
                    if matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b';') {
                        break;
                    }
                    if byte == b'}' {
                        let open = value.rfind("${");
                        let close = value.rfind('}');
                        if open.is_none_or(|open| close.is_some_and(|close| close > open)) {
                            break;
                        }
                    }
                    if byte == b'{' && !value.ends_with('$') {
                        break;
                    }
                    if byte == b'\\' {
                        index += 1;
                        column += 1;
                        if index >= bytes.len() {
                            break;
                        }
                        let escaped = bytes[index];
                        match escaped {
                            b'n' => value.push('\n'),
                            b'r' => value.push('\r'),
                            b't' => value.push('\t'),
                            other => {
                                value.push('\\');
                                value.push(other as char);
                            }
                        }
                        index += 1;
                        column += 1;
                        continue;
                    }
                    if !is_utf8_boundary(source, index) {
                        return Err(ConfigError::at(
                            SourceLocation::new(file, line, column),
                            "invalid UTF-8 in configuration",
                        ));
                    }
                    let character = source[index..].chars().next().unwrap();
                    value.push(character);
                    index += character.len_utf8();
                    column += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Word,
                    value,
                    line: start_line,
                    column: start_column,
                });
            }
        }
    }
    Ok(tokens)
}

fn is_utf8_boundary(source: &str, index: usize) -> bool {
    index == 0 || index == source.len() || source.is_char_boundary(index)
}

#[cfg(test)]
mod tests {
    use super::{TokenKind, lex};

    #[test]
    fn tokenizes_comments_quotes_and_escapes() {
        let tokens = lex(
            "server_name example.com; # comment\n add_header X-Test \"a; b\";",
            std::path::Path::new("test.conf"),
        )
        .unwrap();
        let words: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word))
            .map(|token| token.value.as_str())
            .collect();
        assert_eq!(
            words,
            vec!["server_name", "example.com", "add_header", "X-Test", "a; b"]
        );
    }

    #[test]
    fn tokenizes_single_quotes_and_variables() {
        let tokens = lex(
            "log_format main '$remote_addr \"$request\"';",
            std::path::Path::new("test.conf"),
        )
        .unwrap();
        let words: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word))
            .map(|token| token.value.as_str())
            .collect();
        assert_eq!(
            words,
            vec!["log_format", "main", "$remote_addr \"$request\""]
        );
    }

    #[test]
    fn keeps_braced_variables_in_a_word() {
        let tokens = lex(
            "return 302 https://${host}/$uri;",
            std::path::Path::new("test.conf"),
        )
        .unwrap();
        let words: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word))
            .map(|token| token.value.as_str())
            .collect();
        assert_eq!(words, vec!["return", "302", "https://${host}/$uri"]);
    }

    #[test]
    fn preserves_unknown_escape_sequences() {
        let tokens = lex(
            r#"rewrite "~^/a(\d+)$" /$1; map $host $x { "~^a(\d+)$" "$1"; }"#,
            std::path::Path::new("test.conf"),
        )
        .unwrap();
        let words: Vec<_> = tokens
            .iter()
            .filter(|token| matches!(token.kind, TokenKind::Word))
            .map(|token| token.value.as_str())
            .collect();
        assert!(words.contains(&r"~^/a(\d+)$"));
        assert!(words.contains(&r"~^a(\d+)$"));
    }
}
