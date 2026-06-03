//! Test fixture placeholders.

/// Returns a stable fixture session id used by early core tests.
#[must_use]
pub const fn fixture_session_id() -> &'static str {
    "fixture-session"
}
