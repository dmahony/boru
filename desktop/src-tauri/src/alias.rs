//! Pure alias normalisation and validation rules for desktop identities.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasError {
    InvalidChars,
    TooLong,
    TooShort,
    Reserved,
    Taken,
    Internal,
}

impl AliasError {
    /// Stable IPC-facing error code.
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidChars => "alias_invalid_chars",
            Self::TooLong => "alias_too_long",
            Self::TooShort => "alias_too_short",
            Self::Reserved => "alias_reserved",
            Self::Taken => "alias_taken",
            Self::Internal => "internal",
        }
    }
}

/// Trim outer whitespace and replace each internal whitespace run with one ASCII space.
pub fn normalise_alias(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

const RESERVED: &[&str] = &[
    "system", "admin", "administrator", "boru", "root", "support", "security",
    "moderation", "moderator", "official", "help", "me", "you", "everyone", "all",
    "here", "channel", "group", "server", "relay",
];

/// Normalize and validate an alias, returning the canonical display form.
pub fn validate_alias(input: &str) -> Result<String, AliasError> {
    let alias = normalise_alias(input);
    let count = alias.chars().count();
    if count == 0 {
        return Err(AliasError::TooShort);
    }
    if count > 32 {
        return Err(AliasError::TooLong);
    }

    let mut has_alphanumeric = false;
    for ch in alias.chars() {
        if ch.is_alphabetic() || ch.is_numeric() {
            has_alphanumeric = true;
        } else if !matches!(ch, ' ' | '-' | '_' | '.') {
            return Err(AliasError::InvalidChars);
        }
    }
    if !has_alphanumeric {
        return Err(AliasError::InvalidChars);
    }
    if RESERVED.iter().any(|word| alias.eq_ignore_ascii_case(word)) {
        return Err(AliasError::Reserved);
    }
    Ok(alias)
}

/// Return whether `candidate` is unique among existing aliases, ignoring case.
pub fn is_alias_unique<'a, I>(candidate: &str, existing: I) -> bool
where
    I: IntoIterator<Item = &'a str>,
{
    let candidate = normalise_alias(candidate).to_lowercase();
    !existing
        .into_iter()
        .any(|alias| normalise_alias(alias).to_lowercase() == candidate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_and_collapses_whitespace() {
        assert_eq!(normalise_alias(" \t Ada\n  Lovelace \r"), "Ada Lovelace");
        assert_eq!(validate_alias("  Ada\t  Lovelace ").unwrap(), "Ada Lovelace");
    }

    #[test]
    fn length_boundaries_use_unicode_characters() {
        assert!(validate_alias("a").is_ok());
        assert!(validate_alias(&"a".repeat(32)).is_ok());
        assert_eq!(validate_alias("").unwrap_err(), AliasError::TooShort);
        assert_eq!(validate_alias(&"a".repeat(33)).unwrap_err(), AliasError::TooLong);
    }

    #[test]
    fn accepts_unicode_letters_and_numeric_characters() {
        assert!(validate_alias("東京-é").is_ok());
        assert!(validate_alias("Ⅷ").is_ok());
    }

    #[test]
    fn rejects_disallowed_characters_and_symbols_only() {
        assert_eq!(validate_alias("alice@example").unwrap_err(), AliasError::InvalidChars);
        assert_eq!(validate_alias("-_. ").unwrap_err(), AliasError::InvalidChars);
        assert_eq!(validate_alias("---").unwrap_err(), AliasError::InvalidChars);
    }

    #[test]
    fn rejects_each_reserved_word_case_insensitively() {
        for word in RESERVED {
            assert_eq!(validate_alias(&word.to_uppercase()).unwrap_err(), AliasError::Reserved, "{word}");
        }
    }

    #[test]
    fn uniqueness_ignores_case_and_normalizes_whitespace() {
        let existing = ["Alice", "Ada Lovelace"];
        assert!(!is_alias_unique("aLiCe", existing.iter().copied()));
        assert!(!is_alias_unique(" Ada   Lovelace ", existing.iter().copied()));
        assert!(is_alias_unique("Bob", existing.iter().copied()));
    }

    #[test]
    fn exposes_stable_error_codes() {
        assert_eq!(AliasError::InvalidChars.code(), "alias_invalid_chars");
        assert_eq!(AliasError::TooLong.code(), "alias_too_long");
        assert_eq!(AliasError::TooShort.code(), "alias_too_short");
        assert_eq!(AliasError::Reserved.code(), "alias_reserved");
        assert_eq!(AliasError::Taken.code(), "alias_taken");
        assert_eq!(AliasError::Internal.code(), "internal");
    }
}
