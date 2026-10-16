//! N5 终端主题：外观解析、Settings 外观、Folder/Session 三态覆盖；
//! 命名主题库（内置只读 / 用户可克隆）；以及 D17（只对新开终端生效）。

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
fn palette_resolution_uses_named_theme_and_legacy_overrides() {
    let catalog = yshell_config::ThemeCatalog::default();
    let profile = TerminalProfile {
        color_scheme: Some("dracula".to_owned()),
        foreground: Some("#112233".to_owned()),
        font_family: Some("/tmp/does-not-exist-font.ttf".to_owned()),
        font_size: Some(18),
        fallback_fonts: Some(vec!["/tmp/missing-fallback.ttf".to_owned()]),
        ..TerminalProfile::default()
    };
    let appearance = crate::runtime::theme::terminal_appearance(&profile, &catalog);
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

    let unknown = TerminalProfile {
        color_scheme: Some("not-a-scheme".to_owned()),
        ..TerminalProfile::default()
    };
    assert_eq!(
        crate::runtime::theme::resolve_palette(&unknown, &catalog),
        yshell_terminal::TerminalPalette::DEFAULT
    );
}

#[test]
fn migrate_inline_overrides_creates_user_theme() {
    let temp = tempdir().expect("tempdir");
    let store = ConfigStore::new(temp.path());
    let mut document = ConfigDocument::default();
    document.terminal = TerminalProfile {
        color_scheme: Some("dracula".to_owned()),
        foreground: Some("#112233".to_owned()),
        ..TerminalProfile::default()
    };
    store.save(&document).expect("save");

    let runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    let scheme = runtime
        .config_document
        .terminal
        .color_scheme
        .clone()
        .expect("migrated scheme id");
    assert!(scheme.starts_with("user-"));
    assert!(runtime.config_document.terminal.foreground.is_none());
    assert!(runtime.theme_catalog.contains_user(&scheme));
    let palette = crate::runtime::theme::resolve_palette(
        &runtime.config_document.terminal,
        &runtime.theme_catalog,
    );
    assert_eq!(palette.foreground, TerminalColor::rgb(0x11, 0x22, 0x33));
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
    let font_options = &opened.settings_appearance_font_options;
    assert!(
        font_options
            .last()
            .is_some_and(|label| label == "DejaVu Sans Mono"),
        "bundled DejaVu must remain a preset: {font_options:?}"
    );
    assert_eq!(opened.settings_appearance_font_index, 0);

    let selected = runtime.select_settings_appearance_scheme("nord");
    assert_eq!(selected.settings_appearance_color_scheme_text, "nord");

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
    assert!(persisted.foreground.is_none());
    assert_eq!(persisted.font_size, Some(18));
    assert_eq!(
        persisted.scrollback_lines,
        TerminalProfile::default().scrollback_lines
    );

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
}

#[test]
fn clone_edit_and_delete_user_theme() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let cloned = runtime
        .clone_color_theme("dracula")
        .expect("clone built-in");
    let user_id = cloned.settings_appearance_color_scheme_text.clone();
    assert!(user_id.starts_with("user-"));
    assert!(cloned.theme_editor_open);
    assert!(!cloned.theme_editor_builtin);

    runtime.update_theme_editor_role_color("foreground", "#112233");
    runtime.save_theme_editor().expect("save theme");
    let theme = runtime
        .theme_catalog
        .get_user(&user_id)
        .expect("user theme");
    assert_eq!(theme.foreground, "#112233");

    let listed = runtime.projection();
    assert!(listed
        .settings_appearance_schemes
        .iter()
        .any(|scheme: &ThemeSchemeOption| scheme.id == user_id && !scheme.builtin));

    runtime.delete_color_theme(&user_id).expect("delete");
    assert!(!runtime.theme_catalog.contains_user(&user_id));
    assert_eq!(runtime.settings_appearance.color_scheme, "yshell-default");
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
    let scheme_row = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme_row.state, "explicit");
    assert!(opened
        .folder_editor_appearance_fields
        .iter()
        .all(|field| field.key != "appearance.foreground"
            && field.key != "appearance.background"));
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
    assert_eq!(terminal.color_scheme.as_deref(), Some("nord"));
    assert!(terminal.foreground.is_none());
    assert_eq!(
        terminal.scrollback_lines,
        TerminalProfile::default().scrollback_lines
    );
    let logging = folder.logging.as_ref().expect("logging override");
    assert!(logging.enabled);
    assert_eq!(logging.directory.as_deref(), Some("/var/log/prod"));

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

    let opened = runtime
        .open_folder_editor("folder-prod")
        .expect("open editor");
    let font = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.font_family")
        .expect("font row");
    assert_eq!(font.state, "inherit");
    assert_eq!(font.source_kind, "builtin");

    let scheme_row = opened
        .folder_editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme_row.source_kind, "local-folder");
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
    assert_eq!(loaded.editor_appearance_fields.len(), 4);
    let scheme = loaded
        .editor_appearance_fields
        .iter()
        .find(|field| field.key == "appearance.color_scheme")
        .expect("scheme row");
    assert_eq!(scheme.state, "inherit");

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
    assert!(terminal.foreground.is_none());
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

    runtime.config_document.terminal.color_scheme = Some("dracula".to_owned());
    let first_after = runtime
        .sessions
        .get(&first)
        .expect("session")
        .appearance()
        .palette;
    assert_eq!(first_after, first_appearance);

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
fn scheme_options_include_builtin_and_user_themes() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    let options = crate::runtime::theme::scheme_options(&runtime.theme_catalog);
    assert_eq!(
        options.iter().filter(|option| option.builtin).count(),
        yshell_terminal::COLOR_SCHEMES.len()
    );
    let dracula = options
        .iter()
        .find(|option| option.id == "dracula")
        .expect("dracula option");
    assert_eq!(dracula.name, "Dracula");
    assert!(dracula.builtin);
    assert_eq!(dracula.swatches.len(), 4);

    runtime.clone_color_theme("nord").expect("clone");
    let listed = crate::runtime::theme::scheme_options(&runtime.theme_catalog);
    assert!(listed.iter().any(|option| !option.builtin));
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

    runtime.select_saved_session_by_id("saved-one");
    let session = runtime.projection();
    assert!(!session.saved_folder_selected);
}
