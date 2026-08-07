//! Composer reference grammar for v1.2.
//!
//! Parses the references a developer types in the composer — `@file`,
//! `@file#L42-L91`, `@folder:path`, `@diff`, `@git:ref`, `#symbol` — into a
//! typed, serde-friendly structure the daemon can resolve against the context
//! index and repository. This crate is deliberately dependency-light and
//! pure: it only resolves *intent*. Content lookup happens in the daemon.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The kinds of composer reference a user can type.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Reference {
    /// `@path/to/file.rs` or `@path/to/file.rs#L42-L91`
    File {
        path: String,
        /// Inclusive 1-based line range, e.g. `(42, 91)` for `#L42-L91`.
        range: Option<(u64, u64)>,
    },
    /// `#symbolName`
    Symbol { name: String },
    /// `@folder:path/to/dir`
    Folder { path: String },
    /// `@diff`
    Diff,
    /// `@git:HEAD~1` or `@git:branch`
    Git { reference: String },
    /// `@context` — the full session context summary
    Context,
}

impl Reference {
    /// A human-readable label for display in the resolved-reference surface.
    pub fn display(&self) -> String {
        match self {
            Reference::File { path, range } => match range {
                Some((start, end)) => format!("@{}#L{}-L{}", path, start, end),
                None => format!("@{path}"),
            },
            Reference::Symbol { name } => format!("#{name}"),
            Reference::Folder { path } => format!("@folder:{path}"),
            Reference::Diff => "@diff".into(),
            Reference::Git { reference } => format!("@git:{reference}"),
            Reference::Context => "@context".into(),
        }
    }
}

/// A parsed reference plus the span of raw text it occupied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ParsedReference {
    pub reference: Reference,
    /// Byte offsets into the composer text (`[start, end)`), for the UI to
    /// highlight exactly the typed token.
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Error)]
pub enum ReferenceError {
    #[error("no references found in the composer text")]
    NoReferences,
}

/// A single parsed token with its span, before it is classified.
#[derive(Debug, Clone, PartialEq)]
struct Token {
    start: usize,
    end: usize,
    text: String,
}

/// Extracts the reference tokens from a composer string and classifies them.
///
/// Tokens are whitespace-delimited so prose like "look at @src/auth.rs"
/// resolves `src/auth.rs` without the surrounding words. Line ranges use the
/// `@path#L42-L91` form; a bare `#symbol` inside a token is treated as a
/// symbol reference when it is not part of a file token.
pub fn resolve_refs(text: &str) -> Result<Vec<ParsedReference>, ReferenceError> {
    let mut out = Vec::new();
    for token in tokenize(text) {
        let text = &token.text;
        if let Some(reference) = classify(text) {
            out.push(ParsedReference {
                reference,
                start: token.start,
                end: token.end,
            });
        }
    }
    if out.is_empty() {
        Err(ReferenceError::NoReferences)
    } else {
        Ok(out)
    }
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current: Option<(usize, &str)> = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some((start, _)) = current.take() {
                tokens.push(Token {
                    start,
                    end: index,
                    text: text[start..index].to_string(),
                });
            }
        } else if current.is_none() {
            current = Some((index, ""));
        }
    }
    if let Some((start, _)) = current {
        tokens.push(Token {
            start,
            end: text.len(),
            text: text[start..].to_string(),
        });
    }
    tokens
}

fn classify(token: &str) -> Option<Reference> {
    if token == "@diff" {
        return Some(Reference::Diff);
    }
    if token == "@context" {
        return Some(Reference::Context);
    }
    if let Some(path) = token.strip_prefix("@folder:") {
        let path = path.trim_end_matches([',', ';', ')', ']', '}']);
        if !path.is_empty() {
            return Some(Reference::Folder {
                path: path.to_string(),
            });
        }
        return None;
    }
    if let Some(reference) = token.strip_prefix("@git:") {
        let reference = reference.trim_end_matches([',', ';', ')', ']', '}']);
        if !reference.is_empty() {
            return Some(Reference::Git {
                reference: reference.to_string(),
            });
        }
        return None;
    }
    if let Some(rest) = token.strip_prefix('@') {
        // A file token: `path` or `path#L42-L91`.
        if !rest.is_empty() && !rest.starts_with('#') {
            let (path, range) = split_range(rest);
            let path = path.trim_end_matches([',', ';', ')', ']', '}']);
            if !path.is_empty() {
                return Some(Reference::File {
                    path: path.to_string(),
                    range,
                });
            }
        }
    }
    // A bare `#symbol` (e.g. `#AuthMiddleware`). Tokens like `#tag` inside
    // prose are intentionally included; the daemon decides whether the symbol
    // resolves.
    if let Some(name) = token
        .strip_prefix('#')
        .filter(|name| {
            !name.is_empty()
                && !name.contains(' ')
                && !name.contains('@')
                && !name.ends_with(',')
                && !name.ends_with(';')
        })
    {
        return Some(Reference::Symbol {
            name: name.to_string(),
        });
    }
    None
}

/// Splits `path#L42-L91` into the path and an inclusive line range. Both ends
/// carry an `L` prefix (`#L42-L91`), so each is stripped independently.
fn split_range(token: &str) -> (String, Option<(u64, u64)>) {
    let Some((path, range)) = token.split_once('#') else {
        return (token.to_string(), None);
    };
    let range = match range.split_once('-') {
        Some((start, end)) => {
            let start = start.strip_prefix('L').unwrap_or(start);
            let end = end.strip_prefix('L').unwrap_or(end);
            match (start.parse::<u64>().ok(), end.parse::<u64>().ok()) {
                (Some(start), Some(end)) if start > 0 && start <= end => Some((start, end)),
                _ => None,
            }
        }
        None => None,
    };
    (path.to_string(), range)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Reference> {
        resolve_refs(text)
            .map(|parsed| parsed.into_iter().map(|p| p.reference).collect())
            .unwrap_or_default()
    }

    #[test]
    fn resolves_a_simple_file_reference() {
        assert_eq!(
            kinds("check @src/auth.rs for the bug"),
            vec![Reference::File {
                path: "src/auth.rs".into(),
                range: None,
            }]
        );
    }

    #[test]
    fn resolves_a_line_range() {
        assert_eq!(
            kinds("@src/auth.rs#L42-L91"),
            vec![Reference::File {
                path: "src/auth.rs".into(),
                range: Some((42, 91)),
            }]
        );
    }

    #[test]
    fn resolves_folder_diff_git_and_context() {
        assert_eq!(
            kinds("@folder:src/auth @diff @git:HEAD~1 @context"),
            vec![
                Reference::Folder {
                    path: "src/auth".into(),
                },
                Reference::Diff,
                Reference::Git {
                    reference: "HEAD~1".into(),
                },
                Reference::Context,
            ]
        );
    }

    #[test]
    fn resolves_symbols_and_ignores_plain_words() {
        let result = resolve_refs("refactor #AuthMiddleware and #UserService").unwrap();
        let names: Vec<String> = result
            .into_iter()
            .map(|parsed| match parsed.reference {
                Reference::Symbol { name } => name,
                _ => panic!("expected symbol"),
            })
            .collect();
        assert_eq!(names, vec!["AuthMiddleware", "UserService"]);
    }

    #[test]
    fn spans_match_the_raw_token() {
        let parsed = resolve_refs("see @a/b.rs now").unwrap();
        assert_eq!(parsed[0].start, 4);
        assert_eq!(parsed[0].end, 11);
    }

    #[test]
    fn no_references_is_an_error() {
        assert!(matches!(
            resolve_refs("just some prose with no refs"),
            Err(ReferenceError::NoReferences)
        ));
    }

    #[test]
    fn line_range_must_be_ascending() {
        let parsed = resolve_refs("@a.rs#L91-L42").unwrap();
        assert_eq!(
            parsed[0].reference,
            Reference::File {
                path: "a.rs".into(),
                range: None,
            }
        );
    }
}
