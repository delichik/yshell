//! Guard: application configuration must never come from `YSHELL_*`
//! environment variables.
//!
//! Product configuration (config directory, appearance, language, fonts, local
//! shell, master password / secret store) is read from the configuration files
//! or the `--config-dir` CLI argument. Only test / CI / packaging variables may
//! be read through `env::var*`:
//!
//! * `YSHELL_TEST*` — integration test harness toggles
//! * `YSHELL_LIVE_*` — live SSH/SFTP test targets
//! * `YSHELL_FAKE_AUTH_FAILURE` — auth-failure injection for tests
//! * `YSHELL_APP_LIVE__*` / `YSHELL_EXEC_OK__*` — live-process markers
//! * `YSHELL_RELEASE_VERSION` — packaging version label
//!
//! `RUST_LOG` is a debug-logging control (not part of this scan).

use std::fs;
use std::path::{Path, PathBuf};

/// Environment-variable families that are allowed to be read.
fn is_allowed(name: &str) -> bool {
    matches!(name, "YSHELL_FAKE_AUTH_FAILURE" | "YSHELL_RELEASE_VERSION")
        || name.starts_with("YSHELL_TEST")
        || name.starts_with("YSHELL_LIVE_")
        || name.starts_with("YSHELL_APP_LIVE__")
        || name.starts_with("YSHELL_EXEC_OK__")
}

/// `env::var*`/`set_var`/`remove_var` first-argument literals in one file.
fn env_var_literals(source: &str) -> Vec<(usize, String)> {
    const NEEDLES: [&str; 4] = [
        "env::var(\"",
        "env::var_os(\"",
        "env::set_var(\"",
        "env::remove_var(\"",
    ];
    let mut found = Vec::new();
    for (line_index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // Comments may mention the historical variable names; only code counts.
        if trimmed.starts_with("//") {
            continue;
        }
        for needle in NEEDLES {
            let mut offset = 0;
            while let Some(hit) = line[offset..].find(needle) {
                let start = offset + hit + needle.len();
                if let Some(rest) = line.get(start..) {
                    if let Some(end) = rest.find('"') {
                        found.push((line_index + 1, rest[..end].to_owned()));
                    }
                }
                offset = start;
            }
        }
    }
    found
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Top-level integration test crates (`crates/<crate>/tests/`) may
            // carry throwaway fixtures; the guard targets production code (plus
            // unit tests living under `src/`, e.g. `src/runtime/tests/`).
            if path.file_name().is_some_and(|name| name == "tests")
                && path
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "crates")
            {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn production_code_does_not_read_yshell_config_environment_variables() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let crates_dir = workspace_root.join("crates");
    let mut sources = Vec::new();
    rust_sources(&crates_dir, &mut sources);
    sources.sort();
    assert!(
        sources.len() > 50,
        "expected to scan the workspace crates, found {} files",
        sources.len()
    );

    let mut violations = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        for (line, name) in env_var_literals(&source) {
            if name.starts_with("YSHELL_") && !is_allowed(&name) {
                violations.push(format!("{}:{line}: {name}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "application configuration must not come from YSHELL_* environment variables \
         (use the config files or `--config-dir`):\n{}",
        violations.join("\n")
    );
}

#[test]
fn guard_flags_removed_configuration_variables() {
    // Self-check so a broken scanner cannot silently pass: the removed product
    // configuration variables are flagged while the test/packaging ones are not.
    for removed in [
        "YSHELL_CONFIG_DIR",
        "YSHELL_THEME",
        "YSHELL_ACCENT",
        "YSHELL_LANG",
        "YSHELL_UI_FONT",
        "YSHELL_REDUCE_MOTION",
        "YSHELL_MASTER_PASSWORD",
        "YSHELL_LOCAL_SHELL",
        "YSHELL_FONTS_DIR",
    ] {
        assert!(!is_allowed(removed), "{removed} must not be allowed");
    }
    for allowed in [
        "YSHELL_TEST_SSH_PORT",
        "YSHELL_LIVE_SSH_TARGET",
        "YSHELL_FAKE_AUTH_FAILURE",
        "YSHELL_APP_LIVE__MARKER",
        "YSHELL_EXEC_OK__MARKER",
        "YSHELL_RELEASE_VERSION",
    ] {
        assert!(is_allowed(allowed), "{allowed} must stay allowed");
    }

    let detected = env_var_literals(
        r#"
fn demo() {
    let _ = std::env::var("YSHELL_MASTER_PASSWORD");
    let _ = env::var_os("YSHELL_TEST_SSH_PORT");
}
"#,
    );
    let flagged: Vec<&str> = detected
        .iter()
        .map(|(_, name)| name.as_str())
        .filter(|name| !is_allowed(name))
        .collect();
    assert_eq!(flagged, vec!["YSHELL_MASTER_PASSWORD"]);
}
