//! I18N-1 guard: every literal `set_status_kind("...")` kind used by the
//! runtime must have a branch in `TextFormats.status-message`.
//!
//! Dynamic kind expressions (for example a kind passed through a helper
//! parameter) are deliberately skipped: this test only inspects the first
//! argument when it is a string literal, so it stays a cheap source-level
//! guard without rejecting valid dynamic strings.

use std::fs;
use std::path::PathBuf;

const STATUS_MESSAGE_FUNCTIONS: [&str; 7] = [
    "status-message-core",
    "status-message-tabs",
    "status-message-sftp",
    "status-message-transfer",
    "status-message-terminal",
    "status-message-sessions",
    "status-message-managers",
];

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_text_formats() -> String {
    let path = crate_root().join("../../ui/text_formats.slint");
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

/// Kinds compared against by the `status-message*` function family.
fn template_kinds(source: &str) -> Vec<String> {
    let start = source
        .find("public pure function status-message-core(")
        .expect("status-message-core must exist");
    let end = source
        .find("public pure function status-message(")
        .expect("status-message entry point must exist");
    let mut section_end = source.len();
    // The entry point is the last function of the family; stop at its closing
    // brace (the next function after it is unrelated).
    if let Some(relative) = source[end..].find("\n    }\n") {
        section_end = end + relative + "\n    }\n".len();
    }
    let section = &source[start..section_end.min(source.len())];
    let mut kinds = Vec::new();
    for line in section.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("if kind == \"") else {
            continue;
        };
        let Some(kind) = rest.split('"').next() else {
            continue;
        };
        kinds.push(kind.to_owned());
    }
    kinds
}

/// First-argument kind literals of every `set_status_kind(...)` call.
fn literal_status_kinds(source: &str) -> Vec<String> {
    let mut kinds = Vec::new();
    let mut rest = source;
    while let Some(index) = rest.find("set_status_kind(") {
        rest = &rest[index + "set_status_kind(".len()..];
        let trimmed = rest.trim_start();
        let Some(quoted) = trimmed.strip_prefix('"') else {
            // Dynamic expression: out of scope for this guard.
            continue;
        };
        let Some(kind) = quoted.split('"').next() else {
            continue;
        };
        if !kind.is_empty() {
            kinds.push(kind.to_owned());
        }
    }
    kinds
}

fn runtime_sources() -> Vec<PathBuf> {
    let runtime = crate_root().join("src/runtime");
    let mut paths = Vec::new();
    for entry in fs::read_dir(&runtime)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", runtime.display()))
    {
        let entry = entry.expect("runtime entry");
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "rs") {
            paths.push(path);
        }
    }
    paths.sort();
    assert!(!paths.is_empty(), "no runtime sources found");
    paths
}

#[test]
fn every_literal_status_kind_has_a_status_message_template() {
    let text_formats = read_text_formats();
    for function in STATUS_MESSAGE_FUNCTIONS {
        assert!(
            text_formats.contains(&format!("public pure function {function}(")),
            "TextFormats is missing the `{function}` group"
        );
    }

    let templates = template_kinds(&text_formats);
    assert!(
        templates.len() > 40,
        "expected the status-message templates to be extracted, got {} entries",
        templates.len()
    );

    let mut missing = Vec::new();
    for path in runtime_sources() {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for kind in literal_status_kinds(&source) {
            if !templates.iter().any(|template| template == &kind) {
                missing.push(format!("{}: {kind}", path.display()));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "set_status_kind kinds without a TextFormats.status-message branch:\n{}",
        missing.join("\n")
    );
}

#[test]
fn status_message_kind_branches_are_unique() {
    let mut kinds = template_kinds(&read_text_formats());
    let total = kinds.len();
    kinds.sort();
    kinds.dedup();
    assert_eq!(
        total,
        kinds.len(),
        "duplicate status-message kind branch(es) found"
    );
}
