//! N5 终端主题：外观解析、Settings 外观弹窗、Folder Editor 与 Session Editor
//! 的三态覆盖；以及 D17（只对新开终端生效）。

use tempfile::tempdir;
use yshell_config::{ConfigDocument, ConfigStore, FolderProfile, SessionProfile, TerminalProfile};
use yshell_terminal::{color_scheme, TerminalColor};

use crate::runtime::{ThemeFieldAction, ThemeSchemeOption};

use super::*;

fn store_with_prod_folder(config_dir: &Path) -> ConfigStore {
    let store = ConfigStore::new(config_dir);
    let mut document = ConfigDocument::default();
    let mut root = FolderProfile::new(SAVED_SESSIONS_FOLDER_ID, SAVED_SESSIONS_FOLDER_NAME);
    root.sessions
        .push(SessionProfile::new("saved-one", "One", "one.example.test"));
    let mut prod = FolderProfile::new("folder-prod", "Prod");
    prod.terminal = Some(TerminalProfile {
        color_scheme: Some("nord".to_owned()),
        foreground: Some("#aabbcc".to_owned()),
        ..TerminalProfile::default()
    });
    prod.sessions.push(SessionProfile::new(
        "saved-prod",
        "Prod Box",
        "prod.example.test",
    ));
    root.folders.push(prod);
    document.folders.push(root);
    store.save(&document).expect("save config");
    store
}

#[test]
fn palette_resolution_applies_scheme_then_overrides() {
    let profile = TerminalProfile {
        color_scheme: Some("dracula".to_owned()),
        foreground: Some("#112233".to_owned()),
        font_family: Some("/tmp/does-not-exist-font.ttf".to_owned()),
        font_size: Some(18),
        fallback_fonts: Some(vec!["/tmp/missing-fallback.ttf".to_owned()]),
        ..TerminalProfile::default()
    };
    let appearance = crate::runtime::theme::terminal_appearance(&profile);
    let dracula = color_scheme("dracula").expect("scheme").palette;
    assert_eq!(
        appearance.palette.foreground,
        TerminalColor::rgb(0x11, 0x22, 0x33)
    );
    assert_eq!(appearance.palette.background, dracula.background);
    assert_eq!(appearance.palette.ansi(1), dracula.ansi(1));
    assert_eq!(appearance.font_size, 18.0);
    assert!(
        !appearance.rejected_fonts.is_empty(),
        "the missing font files must be reported"
    );

    // Unknown scheme ids fall back to the built-in palette.
    let unknown = TerminalProfile {
        color_scheme: Some("not-a-scheme".to_owned()),
        ..TerminalProfile::default()
    };
    assert_eq!(
        crate::runtime::theme::resolve_palette(&unknown),
        yshell_terminal::TerminalPalette::DEFAULT
    );
}

#[test]
fn settings_appearance_form_validates_and_persists() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let opened = runtime.open_settings_appearance();
    assert!(opened.settings_appearance_visible);
    assert_eq!(
        opened.settings_appearance_color_scheme_text,
        "yshell-default"
    );
    assert!(!opened.settings_appearance_schemes.is_empty());
    assert!(opened.settings_appearance_preview.is_some());

    // Scheme selection fills the hex fields.
    let selected = runtime.select_settings_appearance_scheme("nord");
    let nord = color_scheme("nord").expect("scheme").palette;
    assert_eq!(
        selected.settings_appearance_foreground_text,
        nord.foreground.to_hex()
    );

    // Explicit overrides + font size.
    runtime.update_settings_appearance_color("foreground", "#112233");
    runtime.update_settings_appearance_font_size("18");
    let saved = runtime.save_settings_appearance().expect("save appearance");
    assert!(saved
        .settings_appearance_status_text
        .contains("Saved the terminal appearance"));
    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload")
        .document
        .terminal;
    assert_eq!(persisted.color_scheme.as_deref(), Some("nord"));
    assert_eq!(persisted.foreground.as_deref(), Some("#112233"));
    assert_eq!(persisted.font_size, Some(18));
    // Inherited scrollback fields are preserved by the save.
    assert_eq!(
        persisted.scrollback_lines,
        TerminalProfile::default().scrollback_lines
    );

    // Invalid hex / font size are rejected without touching the config.
    runtime.update_settings_appearance_color("foreground", "nope");
    let rejected = runtime.save_settings_appearance().expect("save rejected");
    assert!(rejected
        .settings_appearance_status_text
        .contains("appearance.foreground"));
    let after = runtime.config_document.terminal.foreground.clone();
    assert_eq!(after.as_deref(), Some("#112233"));

    runtime.update_settings_appearance_color("foreground", "#445566");
    runtime.update_settings_appearance_font_size("0");
    let rejected = runtime.save_settings_appearance().expect("save rejected");
    assert!(rejected
        .settings_appearance_status_text
        .contains("appearance.font_size"));

    runtime.update_settings_appearance_font_size("16");
    runtime.reset_settings_appearance_defaults();
    let reset = runtime.save_settings_appearance().expect("save reset");
    assert_eq!(
        reset.settings_appearance_color_scheme_text,
        "yshell-default"
    );
    assert!(runtime.config_document.terminal.foreground.is_none());
}

#[test]
fn folder_editor_three_state_round_trip() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let opened = runtime
        .open_folder_editor("folder-prod")
        .expect("open editor");
    assert!(opened.folder_editor_visible);
    assert_eq!(opened.folder_editor_name_text, "Prod");
    // The folder already overrides scheme + foreground; those rows are explicit.
    let scheme_row = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme_row.state, "explicit");
    let background_row = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.background")
        .expect("background row");
    assert_eq!(background_row.state, "inherit");
    // Scrollback rows are group-explicit (the folder stores a terminal profile)
    // and show the parent-resolved values.
    let lines_row = opened
        .folder_editor_terminal_fields
        .iter()
        .find(|field| field.key == "terminal.scrollback_lines")
        .expect("lines row");
    assert_eq!(lines_row.state, "explicit");
    assert_eq!(
        lines_row.value,
        TerminalProfile::default().scrollback_lines.to_string()
    );

    // Edit: explicit selection color + logging on + directory.
    let updated =
        runtime.folder_editor_field_action("appearance.selection", ThemeFieldAction::SetExplicit);
    assert!(updated.folder_editor_visible);
    runtime.update_folder_editor_field("appearance.selection", "#334455");
    runtime.folder_editor_field_action("logging.enabled", ThemeFieldAction::SetExplicit);
    runtime.toggle_folder_editor_logging();
    runtime.update_folder_editor_field("logging.directory", "/var/log/prod");
    let saved = runtime.save_folder_editor().expect("save folder defaults");
    assert!(saved
        .folder_editor_status_text
        .contains("Saved folder defaults"));

    let folder = runtime
        .config_document
        .find_folder("folder-prod")
        .expect("folder");
    let terminal = folder.terminal.as_ref().expect("terminal override");
    assert_eq!(terminal.selection.as_deref(), Some("#334455"));
    assert_eq!(terminal.color_scheme.as_deref(), Some("nord"));
    assert_eq!(terminal.foreground.as_deref(), Some("#aabbcc"));
    // C0 concrete fields are seeded from the parent so the subtree keeps its
    // effective scrollback/logging.
    assert_eq!(
        terminal.scrollback_lines,
        TerminalProfile::default().scrollback_lines
    );
    let logging = folder.logging.as_ref().expect("logging override");
    assert!(logging.enabled);
    assert_eq!(logging.directory.as_deref(), Some("/var/log/prod"));

    // Reset All clears both profiles.
    runtime.reset_all_folder_editor();
    runtime.save_folder_editor().expect("save reset");
    let folder = runtime
        .config_document
        .find_folder("folder-prod")
        .expect("folder");
    assert!(folder.terminal.is_none());
    assert!(folder.logging.is_none());
}

#[test]
fn folder_editor_reports_inherited_sources() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    // Global override for the selection color.
    runtime.config_document.terminal.selection = Some("#010203".to_owned());

    let opened = runtime
        .open_folder_editor("folder-prod")
        .expect("open editor");
    let foreground = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.foreground")
        .expect("foreground row");
    // The folder sets it explicitly, so the row is local.
    assert_eq!(foreground.state, "explicit");
    assert_eq!(foreground.source_kind, "local-folder");

    // Clearing the override falls back to the global value with a source hint.
    let inherit =
        runtime.folder_editor_field_action("appearance.foreground", ThemeFieldAction::SetInherit);
    let foreground = inherit
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.foreground")
        .expect("foreground row");
    assert_eq!(foreground.state, "inherit");
    assert_eq!(foreground.source_kind, "builtin");

    let selection = inherit
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.selection")
        .expect("selection row");
    assert_eq!(selection.source_kind, "global");

    let scheme_row = inherit
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme_row.source_kind, "local-folder");
    assert_eq!(scheme_row.value, "nord");
    assert!(scheme_row.options.iter().any(|option| option == "Nord"));
}

#[test]
fn session_editor_theme_overrides_persist() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    runtime.select_saved_session_by_id("saved-one");
    runtime
        .load_selected_saved_session_into_editor()
        .expect("load editor");
    let loaded = runtime.projection();
    assert_eq!(loaded.editor_terminal_fields.len(), 2);
    assert_eq!(loaded.editor_appearance_fields.len(), 8);
    let scheme = loaded
        .editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme.state, "inherit");

    // Explicit scheme + font size on the session.
    runtime.editor_theme_field_action("appearance.color_scheme", ThemeFieldAction::SetExplicit);
    runtime.select_editor_scheme_index(1); // Dracula
    runtime.update_editor_theme_field("appearance.font_size", "21");
    runtime.editor_theme_field_action("appearance.font_size", ThemeFieldAction::SetExplicit);
    let saved = runtime.save_editor_to_saved_session().expect("save editor");
    assert!(saved.status_text.contains("Saved session editor changes"));

    let session = runtime
        .config_document
        .find_session("saved-one")
        .expect("session");
    let terminal = session.terminal.as_ref().expect("terminal override");
    assert_eq!(terminal.color_scheme.as_deref(), Some("dracula"));
    assert_eq!(terminal.font_size, Some(21));
    // Concrete scrollback is seeded from the resolved chain.
    assert_eq!(
        terminal.scrollback_lines,
        TerminalProfile::default().scrollback_lines
    );

    let resolved = runtime
        .config_document
        .resolve_session("saved-one")
        .expect("resolve");
    assert_eq!(resolved.terminal.color_scheme.as_deref(), Some("dracula"));
    assert_eq!(resolved.terminal.font_size, Some(21));

    // Reset All clears the override again.
    runtime
        .load_selected_saved_session_into_editor()
        .expect("reload");
    runtime.reset_all_editor_theme();
    runtime.save_editor_to_saved_session().expect("save reset");
    assert!(runtime
        .config_document
        .find_session("saved-one")
        .expect("session")
        .terminal
        .is_none());
}

#[test]
fn terminal_appearance_is_frozen_when_the_session_is_created() {
    let temp = tempdir().expect("tempdir");
    let store = tab_test_store(temp.path(), 2);
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    let _ = store;

    let _ = open_saved_tab(&mut runtime, "saved-0");
    let first = runtime.active_session_id.clone().expect("active session");
    let first_appearance = runtime
        .sessions
        .get(&first)
        .expect("session")
        .appearance()
        .palette;
    assert_eq!(first_appearance, yshell_terminal::TerminalPalette::DEFAULT);

    // A later theme change must not touch the existing terminal (D17)…
    runtime.config_document.terminal.color_scheme = Some("dracula".to_owned());
    let first_after = runtime
        .sessions
        .get(&first)
        .expect("session")
        .appearance()
        .palette;
    assert_eq!(first_after, first_appearance);

    // …but the next terminal picks it up.
    let _ = open_saved_tab(&mut runtime, "saved-1");
    let second = runtime.active_session_id.clone().expect("active session");
    let dracula = color_scheme("dracula").expect("scheme").palette;
    assert_eq!(
        runtime
            .sessions
            .get(&second)
            .expect("session")
            .appearance()
            .palette,
        dracula
    );
}

#[test]
fn scheme_options_carry_id_name_and_swatches() {
    let options: Vec<ThemeSchemeOption> = crate::runtime::theme::scheme_options();
    assert_eq!(options.len(), yshell_terminal::COLOR_SCHEMES.len());
    let dracula = options
        .iter()
        .find(|option| option.id == "dracula")
        .expect("dracula option");
    assert_eq!(dracula.name, "Dracula");
    assert_eq!(dracula.swatches.len(), 4);
}

#[test]
fn folder_editor_menu_requires_a_selected_folder() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    let initial = runtime.projection();
    assert!(!initial.saved_folder_selected);

    runtime.select_saved_session_by_id("folder-prod");
    let folder = runtime.projection();
    assert!(folder.saved_folder_selected);
    assert!(!folder.saved_session_selection_kind_text.is_empty());

    runtime.select_saved_session_by_id("saved-one");
    let session = runtime.projection();
    assert!(!session.saved_folder_selected);
}
