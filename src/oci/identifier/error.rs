// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 The Grimoire Authors

//! Identifier parse errors.

/// An error that occurred while parsing an OCI identifier string.
// `input` is untrusted — it comes from a hand-authored `grimoire.toml` or a
// CLI argument — and this message goes to a terminal. `escape_debug` renders
// control bytes and the bidi/zero-width format characters as `\u{…}`; it is
// the identity function for every well-formed identifier, so the shipped
// message text is unchanged.
#[derive(Debug, Clone, thiserror::Error)]
#[error("invalid identifier '{}': {kind}", .input.escape_debug())]
#[non_exhaustive]
pub struct IdentifierError {
    /// The raw input that failed to parse.
    pub input: String,
    /// The specific reason parsing failed.
    pub kind: IdentifierErrorKind,
}

impl IdentifierError {
    pub fn new(input: impl Into<String>, kind: IdentifierErrorKind) -> Self {
        Self {
            input: input.into(),
            kind,
        }
    }
}

/// The specific reason an identifier string failed to parse.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum IdentifierErrorKind {
    /// The input string was empty.
    #[error("identifier cannot be empty")]
    Empty,
    /// The repository portion contains uppercase characters.
    #[error("repository must be lowercase")]
    UppercaseRepository,
    /// The repository portion exceeds the 255-character limit.
    #[error("repository exceeds 255-character limit")]
    RepositoryTooLong,
    /// The digest string is invalid (unsupported algorithm, wrong length, non-hex chars).
    #[error("invalid digest format")]
    DigestInvalidFormat,
    /// The identifier format is invalid (cannot be parsed).
    #[error("invalid format")]
    InvalidFormat,
    /// The identifier does not contain an explicit registry.
    #[error("identifier must include an explicit registry (e.g. 'ghcr.io/org/tool:1.0', not 'tool:1.0')")]
    MissingRegistry,
    /// The identifier contains a directory traversal segment (`.` or `..`).
    #[error("identifier must not use '.' or '..' as a path segment")]
    DirectoryTraversal,
    /// The repository portion violates the OCI name-component grammar
    /// (illegal character, or a misplaced/doubled separator).
    #[error(
        "repository must match the OCI name grammar: lowercase [a-z0-9] runs joined by '.', '_', '__', or '-', with no leading, trailing, or doubled separator"
    )]
    RepositoryGrammar,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_escapes_a_hostile_input() {
        // `input` is echoed verbatim from a hand-authored `grimoire.toml`,
        // which arrives with a `git clone`. Raw, a control sequence in it
        // reaches the terminal of anyone running a config-loading command.
        // U+202E is not `char::is_control`, so no upstream guard stops it.
        let err = IdentifierError::new("\u{1b}[2Jghcr.io/a/B:1", IdentifierErrorKind::UppercaseRepository);
        assert_eq!(
            err.to_string(),
            r"invalid identifier '\u{1b}[2Jghcr.io/a/B:1': repository must be lowercase"
        );
        let err = IdentifierError::new("ghcr.io/a/B\u{202e}c:1", IdentifierErrorKind::UppercaseRepository);
        assert_eq!(
            err.to_string(),
            r"invalid identifier 'ghcr.io/a/B\u{202e}c:1': repository must be lowercase"
        );

        // Nothing to escape → byte-identical to the shipped message.
        let err = IdentifierError::new("ghcr.io/a/B:1", IdentifierErrorKind::UppercaseRepository);
        assert_eq!(
            err.to_string(),
            "invalid identifier 'ghcr.io/a/B:1': repository must be lowercase"
        );
    }
}
