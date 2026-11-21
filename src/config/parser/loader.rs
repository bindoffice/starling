use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::ast::{ConfigDocument, Directive};
use crate::error::{ConfigError, ConfigResult};

use super::lexer::lex;
use super::syntax::Parser;

pub struct ConfigLoader {
    prefix: PathBuf,
    include_prefix: PathBuf,
    included_files: Vec<PathBuf>,
    active_includes: HashSet<PathBuf>,
}

impl ConfigLoader {
    pub fn new(prefix: impl Into<PathBuf>) -> Self {
        Self {
            prefix: prefix.into(),
            include_prefix: PathBuf::new(),
            included_files: Vec::new(),
            active_includes: HashSet::new(),
        }
    }

    pub fn load(mut self, path: &Path) -> ConfigResult<ConfigDocument> {
        let resolved = self.resolve_path(path);
        self.include_prefix = resolved
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let directives = self.load_file(&resolved)?;
        Ok(ConfigDocument {
            directives,
            files: self.included_files,
        })
    }

    pub fn load_with_inline(mut self, path: &Path, inline: &str) -> ConfigResult<ConfigDocument> {
        let resolved = self.resolve_path(path);
        self.include_prefix = resolved
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let mut directives = self.load_file(&resolved)?;
        if !inline.trim().is_empty() {
            let inline_path = PathBuf::from("<command line>");
            let tokens = lex(inline, &inline_path)?;
            let mut parser = Parser::new(tokens, inline_path.clone());
            let mut extra = parser.parse_directives(false)?;
            self.expand_includes(&mut extra)?;
            directives.extend(extra);
            self.included_files.push(inline_path);
        }
        Ok(ConfigDocument {
            directives,
            files: self.included_files,
        })
    }

    fn resolve_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.prefix.join(path)
        }
    }

    fn resolve_include_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.include_prefix.join(path)
        }
    }

    fn load_file(&mut self, path: &Path) -> ConfigResult<Vec<Directive>> {
        let canonical = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !self.active_includes.insert(canonical.clone()) {
            return Err(ConfigError::new(format!(
                "circular include detected: {}",
                path.display()
            )));
        }
        if !self.included_files.contains(&canonical) {
            self.included_files.push(canonical.clone());
        }

        let source = fs::read_to_string(path).map_err(|err| ConfigError::io(path, err))?;
        let tokens = lex(&source, path)?;
        let mut parser = Parser::new(tokens, path.to_path_buf());
        let mut directives = parser.parse_directives(false)?;
        drop(parser);
        self.expand_includes(&mut directives)?;
        self.active_includes.remove(&canonical);
        Ok(directives)
    }

    fn expand_includes(&mut self, directives: &mut Vec<Directive>) -> ConfigResult<()> {
        let mut expanded = Vec::with_capacity(directives.len());
        for mut directive in directives.drain(..) {
            if directive.name == "include" && !directive.has_block {
                if directive.args.len() != 1 {
                    return Err(ConfigError::at(
                        directive.location,
                        "invalid number of arguments in \"include\" directive",
                    ));
                }
                let pattern = directive.args[0].clone();
                let resolved = self.resolve_include_path(Path::new(&pattern));
                let pattern_text = resolved.to_string_lossy().to_string();
                let paths = expand_glob(&pattern_text)?;
                if paths.is_empty() {
                    let has_glob = pattern_text.contains(['*', '?', '[']);
                    if !has_glob {
                        return Err(ConfigError::at(
                            directive.location,
                            format!(
                                "open() \"{}\" failed (2: No such file or directory)",
                                resolved.display()
                            ),
                        ));
                    }
                    continue;
                }
                for path in paths {
                    let mut included = self.load_file(&path)?;
                    expanded.append(&mut included);
                }
            } else {
                self.expand_includes(&mut directive.children)?;
                expanded.push(directive);
            }
        }
        *directives = expanded;
        Ok(())
    }
}

fn expand_glob(pattern: &str) -> ConfigResult<Vec<PathBuf>> {
    if !pattern.contains(['*', '?', '[']) {
        return Ok(vec![PathBuf::from(pattern)]);
    }

    let path = Path::new(pattern);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_pattern = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ConfigError::new(format!("invalid include path: {pattern}")))?;
    let matcher = regex::Regex::new(&glob_to_regex(file_pattern))
        .map_err(|err| ConfigError::new(format!("invalid include pattern: {err}")))?;
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(ConfigError::io(parent, err)),
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| ConfigError::io(parent, err))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if matcher.is_match(name) {
            matches.push(entry.path());
        }
    }
    matches.sort();
    Ok(matches)
}

fn glob_to_regex(pattern: &str) -> String {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '*' => regex.push_str(".*"),
            '?' => regex.push('.'),
            '[' => {
                regex.push('[');
                for next in chars.by_ref() {
                    regex.push(next);
                    if next == ']' {
                        break;
                    }
                }
            }
            other => regex.push_str(&regex::escape(&other.to_string())),
        }
    }
    regex.push('$');
    regex
}
