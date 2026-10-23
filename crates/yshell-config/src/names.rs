//! Display-name validation for on-disk session folders/files and key labels.

/// Characters that must never appear in a display name used as a path segment.
const FORBIDDEN_CHARS: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Error when a display name cannot be used as a filesystem segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Reserved(String),
    ForbiddenChar { name: String, ch: char },
    ControlChar(String),
    Collision { name: String },
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("name must not be empty"),
            Self::Reserved(name) => write!(f, "name `{name}` is reserved"),
            Self::ForbiddenChar { name, ch } => {
                write!(f, "name `{name}` must not contain `{ch}`")
            }
            Self::ControlChar(name) => {
                write!(f, "name `{name}` must not contain control characters")
            }
            Self::Collision { name } => {
                write!(f, "name `{name}` already exists in this location")
            }
        }
    }
}

impl std::error::Error for NameError {}

/// Trims and validates a display name for use as a single path segment.
pub fn validate_entry_name(name: &str) -> Result<String, NameError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NameError::Empty);
    }
    if trimmed == "." || trimmed == ".." {
        return Err(NameError::Reserved(trimmed.to_owned()));
    }
    // Windows rejects names that end with `.` or space even after trim of the
    // outer string (e.g. `"a. "` → `"a."` after trim is still invalid).
    if trimmed.ends_with('.') {
        return Err(NameError::Reserved(trimmed.to_owned()));
    }
    if let Some(ch) = trimmed.chars().find(|ch| FORBIDDEN_CHARS.contains(ch)) {
        return Err(NameError::ForbiddenChar {
            name: trimmed.to_owned(),
            ch,
        });
    }
    if trimmed.chars().any(|ch| ch.is_control()) {
        return Err(NameError::ControlChar(trimmed.to_owned()));
    }
    Ok(trimmed.to_owned())
}

/// Rewrites a display name into a filesystem-safe path segment.
///
/// Forbidden / control characters become `-`; empty results fall back to
/// `fallback` (defaulting to `"session"`).
#[must_use]
pub fn sanitize_entry_name(name: &str, fallback: &str) -> String {
    let rewritten: String = name
        .trim()
        .chars()
        .map(|ch| {
            if FORBIDDEN_CHARS.contains(&ch) || ch.is_control() {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let collapsed = rewritten
        .split_whitespace()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let trimmed = collapsed.trim_matches(|ch: char| ch == '-' || ch == '.' || ch.is_whitespace());
    if let Ok(valid) = validate_entry_name(trimmed) {
        return valid;
    }
    let fallback = fallback.trim();
    if let Ok(valid) = validate_entry_name(fallback) {
        return valid;
    }
    "session".to_owned()
}

/// Case-insensitive equality for sibling uniqueness on Windows-friendly trees.
#[must_use]
pub fn names_equal_ignore_case(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

/// Returns whether `candidate` collides with any name in `existing` (case-insensitive).
#[must_use]
pub fn name_collides<'a>(
    candidate: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> bool {
    existing
        .into_iter()
        .any(|name| names_equal_ignore_case(name, candidate))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_forbidden() {
        assert!(matches!(validate_entry_name("  "), Err(NameError::Empty)));
        assert!(matches!(
            validate_entry_name("a/b"),
            Err(NameError::ForbiddenChar { .. })
        ));
        assert!(matches!(
            validate_entry_name(".."),
            Err(NameError::Reserved(_))
        ));
    }

    #[test]
    fn collision_is_case_insensitive() {
        assert!(name_collides("Web", ["web", "api"]));
        assert!(!name_collides("db", ["web", "api"]));
    }

    #[test]
    fn sanitize_replaces_forbidden_chars() {
        assert_eq!(
            sanitize_entry_name("alice@example.com:2200", "session"),
            "alice@example.com-2200"
        );
        assert_eq!(sanitize_entry_name("  ", "host"), "host");
    }
}
