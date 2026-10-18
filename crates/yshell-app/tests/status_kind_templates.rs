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

/// Producers that write an i18n status kind. The first argument is the kind in
/// every call site covered by this guard; dynamic expressions are skipped.
const STATUS_KIND_SETTERS: [&str; 7] = [
    "set_status_kind",
    "set_private_keys_status_kind",
    "set_private_keys_status_feedback",
    "set_host_keys_status_kind",
    "set_host_keys_status_feedback",
    "set_proxies_status_kind",
    "set_folder_editor_status",
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

/// First-argument kind literals of every `set_status_kind(...)`-style call.
fn literal_status_kinds(source: &str) -> Vec<String> {
    let mut kinds = Vec::new();
    for setter in STATUS_KIND_SETTERS {
        let needle = format!("{setter}(");
        let mut rest = source;
        while let Some(index) = rest.find(&needle) {
            rest = &rest[index + needle.len()..];
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

/// Runtime sources plus the bootstrap host, which also writes status kinds.
fn status_source_files() -> Vec<PathBuf> {
    let mut paths = runtime_sources();
    paths.push(crate_root().join("src/bootstrap.rs"));
    paths
}

/// Direct `.status_text = ...` writes in the files this migration covers.
fn raw_status_text_writes(source: &str) -> Vec<String> {
    let mut hits = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") {
            continue;
        }
        let looks_like_write = (trimmed.starts_with("self.") || trimmed.starts_with("runtime."))
            && trimmed.contains("status_text =");
        if !looks_like_write {
            continue;
        }
        // The page helpers intentionally store the English fallback field.
        if trimmed.ends_with("status_text = text;") {
            continue;
        }
        hits.push(format!("line {}: {trimmed}", index + 1));
    }
    hits
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
    for path in status_source_files() {
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
        "status kind producer(s) without a TextFormats.status-message branch:\n{}",
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

#[test]
fn scoped_producers_do_not_write_raw_status_text() {
    // I18N-1 scope: manager/page statuses and the bootstrap host. New producers
    // must go through `set_*_status_kind`/`set_status_kind`, carrying a kind.
    let scoped = [
        crate_root().join("src/runtime/auth.rs"),
        crate_root().join("src/runtime/proxies.rs"),
        crate_root().join("src/runtime/folder_editor.rs"),
        crate_root().join("src/bootstrap.rs"),
    ];
    let mut violations = Vec::new();
    for path in scoped {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for hit in raw_status_text_writes(&source) {
            violations.push(format!("{}: {hit}", path.display()));
        }
    }
    assert!(
        violations.is_empty(),
        "raw status_text writes remain (use a status kind + params):\n{}",
        violations.join("\n")
    );
}
