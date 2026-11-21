use std::path::PathBuf;

use crate::config::ast::Directive;
use crate::error::{ConfigError, ConfigResult, SourceLocation};

use super::lexer::{Token, TokenKind};

pub(super) struct Parser {
    tokens: Vec<Token>,
    index: usize,
    file: PathBuf,
}

impl Parser {
    pub(super) fn new(tokens: Vec<Token>, file: PathBuf) -> Self {
        Self {
            tokens,
            index: 0,
            file,
        }
    }

    pub(super) fn parse_directives(&mut self, in_block: bool) -> ConfigResult<Vec<Directive>> {
        let mut directives = Vec::new();
        loop {
            let Some(token) = self.peek() else {
                if in_block {
                    return Err(ConfigError::at(
                        SourceLocation::new(&self.file, self.last_line(), 1),
                        "unexpected end of file, expecting \"}\"",
                    ));
                }
                break;
            };

            if token.kind == TokenKind::CloseBrace {
                if !in_block {
                    return Err(ConfigError::at(self.location(token), "unexpected \"}\""));
                }
                self.index += 1;
                break;
            }
            if token.kind == TokenKind::Semicolon {
                return Err(ConfigError::at(self.location(token), "unexpected \";\""));
            }
            if token.kind == TokenKind::OpenBrace {
                return Err(ConfigError::at(self.location(token), "unexpected \"{\""));
            }

            let name_token = self.next().expect("peeked token");
            let mut args = Vec::new();
            loop {
                let Some(next) = self.peek() else {
                    return Err(ConfigError::at(
                        SourceLocation::new(&self.file, name_token.line, name_token.column),
                        format!(
                            "unexpected end of file, expecting \";\" or \"}}\" in \"{}\" directive",
                            name_token.value
                        ),
                    ));
                };
                match next.kind {
                    TokenKind::Word => {
                        args.push(next.value.clone());
                        self.index += 1;
                    }
                    TokenKind::Semicolon => {
                        self.index += 1;
                        directives.push(Directive {
                            name: name_token.value,
                            args,
                            children: Vec::new(),
                            location: SourceLocation::new(
                                &self.file,
                                name_token.line,
                                name_token.column,
                            ),
                            has_block: false,
                        });
                        break;
                    }
                    TokenKind::OpenBrace => {
                        self.index += 1;
                        let children = self.parse_directives(true)?;
                        directives.push(Directive {
                            name: name_token.value,
                            args,
                            children,
                            location: SourceLocation::new(
                                &self.file,
                                name_token.line,
                                name_token.column,
                            ),
                            has_block: true,
                        });
                        break;
                    }
                    TokenKind::CloseBrace => {
                        return Err(ConfigError::at(
                            self.location(next),
                            format!(
                                "unexpected \"}}\", expecting \";\" or \"{{\" in \"{}\" directive",
                                name_token.value
                            ),
                        ));
                    }
                }
            }
        }
        Ok(directives)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.index).cloned();
        if token.is_some() {
            self.index += 1;
        }
        token
    }

    fn location(&self, token: &Token) -> SourceLocation {
        SourceLocation::new(&self.file, token.line, token.column)
    }

    fn last_line(&self) -> usize {
        self.tokens.last().map(|token| token.line).unwrap_or(1)
    }
}
