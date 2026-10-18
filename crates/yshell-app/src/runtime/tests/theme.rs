//! N5 终端主题：外观解析、Settings 外观、Folder/Session 三态覆盖；
//! 命名主题库（内置只读 / 用户可克隆）；以及 D17（只对新开终端生效）。

use tempfile::tempdir;
use yshell_config::{ConfigDocument, ConfigStore, FolderProfile, SessionProfile, TerminalProfile};
use yshell_terminal::{color_scheme, TerminalCell, TerminalColor, TerminalPalette};

use crate::runtime::ThemeFieldAction;

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
        backup_font: Some("/tmp/missing-fallback.ttf".to_owned()),
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
        opened.settings_appearance_color_scheme_name_text,
        "YShell Default"
    );
    assert!(opened.settings_appearance_color_scheme_builtin);
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
    assert_eq!(runtime.settings_appearance.color_scheme, "nord");
    // The summary always reflects the *global default*, not the unsaved draft.
    assert_eq!(
        selected.settings_appearance_color_scheme_name_text,
        "YShell Default"
    );

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
        reset.settings_appearance_color_scheme_name_text,
        "YShell Default"
    );
}

#[test]
fn settings_appearance_backup_font_is_single_value() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let first = temp.path().join("first-fallback.ttf");
    let second = temp.path().join("second-fallback.ttf");
    std::fs::write(&first, b"not a real font").expect("write first");
    std::fs::write(&second, b"not a real font").expect("write second");
    let first_text = first.display().to_string();
    let second_text = second.display().to_string();

    // A pasted multi-line value collapses to the first font at the save boundary.
    runtime.update_settings_appearance_backup_font(&format!("{first_text}\n{second_text}"));
    let saved = runtime.save_settings_appearance().expect("save appearance");
    assert_eq!(saved.settings_appearance_backup_font_text, first_text);

    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload")
        .document
        .terminal;
    assert_eq!(persisted.backup_font.as_deref(), Some(first_text.as_str()));
    let toml = std::fs::read_to_string(ConfigStore::new(temp.path()).config_file()).expect("read");
    assert!(toml.contains("backup_font ="));
    assert!(!toml.contains("fallback_fonts"));

    // The picker replaces the previous value; Clear empties the field.
    runtime.set_settings_appearance_backup_font(&second_text);
    assert_eq!(runtime.settings_appearance.backup_font_text, second_text);
    let cleared = runtime.clear_settings_appearance_backup_font();
    assert_eq!(cleared.settings_appearance_backup_font_text, "");
}

#[test]
fn legacy_fallback_fonts_load_as_one_backup_font() {
    let temp = tempdir().expect("tempdir");
    let config = r#"
schema_version = 2

[terminal]
font_family = "JetBrains Mono"
font_size = 15
fallback_fonts = ["/tmp/first.ttf", "/tmp/second.ttf"]
"#;
    std::fs::write(temp.path().join("config.toml"), config).expect("write config");

    let runtime = AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    assert_eq!(
        runtime.config_document.terminal.font_family.as_deref(),
        Some("JetBrains Mono")
    );
    assert_eq!(runtime.config_document.terminal.font_size, Some(15));
    assert_eq!(
        runtime.config_document.terminal.backup_font.as_deref(),
        Some("/tmp/first.ttf")
    );
    // Startup persists the migrated single-value form (one-shot warning).
    let toml = std::fs::read_to_string(temp.path().join("config.toml")).expect("read config");
    assert!(toml.contains("backup_font = \"/tmp/first.ttf\""));
    assert!(!toml.contains("fallback_fonts"));
}

#[test]
fn clone_edit_and_delete_user_theme() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let cloned = runtime
        .clone_color_theme("dracula")
        .expect("clone built-in");
    let user_id = cloned
        .theme_manager_rows
        .iter()
        .find(|row| row.selected)
        .expect("selected clone")
        .id
        .clone();
    assert!(user_id.starts_with("user-"));
    assert!(cloned.theme_editor_open);
    assert!(!cloned.theme_editor_builtin);
    assert!(cloned
        .theme_manager_rows
        .iter()
        .any(|row| row.id == user_id && row.selected && !row.builtin));

    runtime.update_theme_editor_role_color("foreground", "#112233");
    runtime.save_theme_editor().expect("save theme");
    let theme = runtime
        .theme_catalog
        .get_user(&user_id)
        .expect("user theme");
    assert_eq!(theme.foreground, "#112233");

    let listed = runtime.projection();
    assert!(listed
        .theme_manager_rows
        .iter()
        .any(|row: &crate::runtime::ThemeManagerRowData| {
            row.id == user_id && !row.builtin && row.selected
        }));

    runtime.delete_color_theme(&user_id).expect("delete");
    assert!(!runtime.theme_catalog.contains_user(&user_id));
    assert_eq!(runtime.settings_appearance.color_scheme, "yshell-default");
    let after = runtime.projection();
    assert!(after
        .theme_manager_rows
        .iter()
        .any(|row| row.id == "yshell-default" && row.current && row.selected));
}

#[test]
fn theme_manager_lists_builtin_and_user_themes_with_the_default_flagged() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let opened = runtime.open_theme_manager();
    assert!(opened.theme_manager_visible);
    assert_eq!(
        opened.theme_manager_builtin_count,
        i32::try_from(yshell_terminal::COLOR_SCHEMES.len()).expect("builtin count")
    );
    assert_eq!(opened.theme_manager_user_count, 0);
    assert_eq!(
        opened.theme_manager_rows.len(),
        yshell_terminal::COLOR_SCHEMES.len()
    );
    let current = opened
        .theme_manager_rows
        .iter()
        .find(|row| row.current)
        .expect("current default row");
    assert_eq!(current.id, "yshell-default");
    assert!(current.builtin);
    assert!(current.selected);
    assert_eq!(
        opened.theme_manager_selected_index,
        i32::try_from(
            opened
                .theme_manager_rows
                .iter()
                .position(|row| row.id == "yshell-default")
                .expect("default row index")
        )
        .expect("index")
    );
    assert_eq!(opened.theme_manager_default_name_text, "YShell Default");
    assert!(opened.theme_manager_preview.is_some());

    let cloned = runtime.clone_color_theme("nord").expect("clone built-in");
    let user_id = cloned
        .theme_manager_rows
        .iter()
        .find(|row| row.selected)
        .expect("selected clone")
        .id
        .clone();
    assert!(user_id.starts_with("user-"));
    assert_eq!(cloned.theme_manager_user_count, 1);
    assert_eq!(cloned.theme_manager_rows.len(), opened.theme_manager_rows.len() + 1);
    let cloned_row = cloned
        .theme_manager_rows
        .iter()
        .find(|row| row.id == user_id)
        .expect("cloned row");
    assert!(!cloned_row.builtin);
    assert!(cloned_row.selected);
    assert!(!cloned_row.current, "cloning must not change the default");
    assert!(cloned.theme_editor_open);
    assert!(!cloned.theme_editor_builtin);
}

#[test]
fn theme_manager_row_selection_updates_preview_and_apply_sets_default() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.note_ui_scale_factor(1.0);
    let opened = runtime.open_theme_manager();
    let dracula_index = opened
        .theme_manager_rows
        .iter()
        .position(|row| row.id == "dracula")
        .expect("dracula row");
    let dracula_index = i32::try_from(dracula_index).expect("index");

    let selected = runtime.select_theme_manager_row(dracula_index);
    assert_eq!(selected.theme_manager_selected_index, dracula_index);
    assert!(!selected.theme_manager_selected_is_default);
    assert!(selected
        .theme_manager_rows
        .iter()
        .any(|row| row.id == "dracula" && row.selected && !row.current));

    let dracula = color_scheme("dracula").expect("dracula scheme").palette;
    let frame = selected
        .theme_manager_preview
        .as_ref()
        .expect("manager preview frame");
    let columns = u16::try_from(frame.width / frame.cell_width).expect("columns");
    let rows = u16::try_from(frame.height / frame.cell_height).expect("rows");
    assert_eq!(
        frame_cell_pixel(frame, columns - 1, rows - 1),
        color_rgba(dracula.background),
        "row click must preview the selected theme without saving"
    );

    let applied = runtime
        .apply_theme_manager_selection()
        .expect("apply default theme");
    assert_eq!(
        runtime.config_document.terminal.color_scheme.as_deref(),
        Some("dracula")
    );
    assert!(applied.theme_manager_selected_is_default);
    assert_eq!(applied.theme_manager_default_name_text, "Dracula");
    assert!(applied
        .theme_manager_rows
        .iter()
        .any(|row| row.id == "dracula" && row.current && row.selected));

    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload")
        .document
        .terminal;
    assert_eq!(persisted.color_scheme.as_deref(), Some("dracula"));
}

#[test]
fn theme_manager_clone_edit_and_delete_require_confirmation() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.note_ui_scale_factor(1.0);
    runtime.open_theme_manager();
    let cloned = runtime.clone_color_theme("dracula").expect("clone");
    let user_id = cloned
        .theme_manager_rows
        .iter()
        .find(|row| row.selected)
        .expect("selected clone")
        .id
        .clone();
    assert!(cloned.theme_editor_open);

    // Editing the draft re-renders the preview on the same projection.
    let edited = runtime.update_theme_editor_role_color("background", "#112233");
    let frame = edited
        .theme_manager_preview
        .as_ref()
        .expect("preview while editing");
    let columns = u16::try_from(frame.width / frame.cell_width).expect("columns");
    let rows = u16::try_from(frame.height / frame.cell_height).expect("rows");
    assert_eq!(
        frame_cell_pixel(frame, columns - 1, rows - 1),
        color_rgba(TerminalColor::rgb(0x11, 0x22, 0x33)),
        "preview must follow the editor draft"
    );
    runtime.save_theme_editor().expect("save theme");
    let saved = runtime.projection();
    assert!(saved
        .theme_manager_rows
        .iter()
        .any(|row| row.id == user_id && row.name == "Dracula Copy" && !row.builtin));

    // Built-ins never enter the delete-confirmation path.
    let builtin = runtime.request_delete_color_theme("dracula");
    assert!(!builtin.theme_manager_remove_confirm_visible);
    assert!(runtime.delete_color_theme("dracula").is_err());

    // User themes ask first; Cancel keeps the catalog untouched.
    let requested = runtime.request_delete_color_theme(&user_id);
    assert!(requested.theme_manager_remove_confirm_visible);
    assert_eq!(requested.theme_manager_remove_name_text, "Dracula Copy");
    assert_eq!(requested.theme_manager_remove_reference_count, 0);
    let canceled = runtime.cancel_delete_color_theme();
    assert!(!canceled.theme_manager_remove_confirm_visible);
    assert!(runtime.theme_catalog.contains_user(&user_id));

    runtime.request_delete_color_theme(&user_id);
    let deleted = runtime.confirm_delete_color_theme().expect("delete confirmed");
    assert!(!runtime.theme_catalog.contains_user(&user_id));
    assert!(!deleted
        .theme_manager_rows
        .iter()
        .any(|row| row.id == user_id));
    assert!(deleted.theme_manager_visible);
    assert_eq!(deleted.theme_manager_user_count, 0);
    assert!(!deleted.theme_manager_remove_confirm_visible);
}

#[test]
fn theme_manager_delete_retargets_references_to_the_default() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.open_theme_manager();
    runtime.clone_color_theme("nord").expect("clone");
    let user_id = runtime
        .theme_manager_selection_id();
    runtime
        .apply_theme_manager_selection()
        .expect("apply cloned default");
    // A folder and a saved session also reference the user theme.
    let folder = runtime
        .config_document
        .find_folder_mut("folder-prod")
        .expect("prod folder");
    folder
        .terminal
        .get_or_insert_with(TerminalProfile::default)
        .color_scheme = Some(user_id.clone());
    let session = runtime
        .config_document
        .find_session_mut("saved-prod")
        .expect("prod session");
    session
        .terminal
        .get_or_insert_with(TerminalProfile::default)
        .color_scheme = Some(user_id.clone());
    assert_eq!(
        crate::runtime::theme::theme_reference_count(&runtime.config_document, &user_id),
        3,
        "global default + folder + session references"
    );

    let requested = runtime.request_delete_color_theme(&user_id);
    assert_eq!(requested.theme_manager_remove_reference_count, 3);
    let deleted = runtime
        .confirm_delete_color_theme()
        .expect("delete and retarget");
    assert!(!runtime.theme_catalog.contains_user(&user_id));
    assert_eq!(
        crate::runtime::theme::theme_reference_count(&runtime.config_document, &user_id),
        0,
        "no orphaned theme references may remain"
    );
    assert_eq!(
        runtime.config_document.terminal.color_scheme.as_deref(),
        Some("yshell-default")
    );
    assert_eq!(
        runtime
            .config_document
            .find_folder("folder-prod")
            .expect("folder")
            .terminal
            .as_ref()
            .expect("folder terminal")
            .color_scheme
            .as_deref(),
        Some("yshell-default")
    );
    assert_eq!(
        runtime
            .config_document
            .find_session("saved-prod")
            .expect("session")
            .terminal
            .as_ref()
            .expect("session terminal")
            .color_scheme
            .as_deref(),
        Some("yshell-default")
    );
    assert_eq!(
        deleted.theme_manager_default_name_text, "YShell Default",
        "the manager falls back to the built-in default after deletion"
    );
    let persisted = ConfigStore::new(temp.path())
        .load_or_recover()
        .expect("reload")
        .document;
    assert_eq!(
        persisted.terminal.color_scheme.as_deref(),
        Some("yshell-default")
    );
    assert_eq!(
        crate::runtime::theme::theme_reference_count(&persisted, &user_id),
        0
    );
}

#[test]
fn theme_manager_opens_from_settings_and_closes_back_to_it() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let settings = runtime.open_settings_appearance();
    assert!(settings.settings_appearance_visible);
    let manager = runtime.open_theme_manager();
    assert!(manager.theme_manager_visible);
    assert!(manager.settings_appearance_visible, "Settings stays behind");
    assert_eq!(manager.theme_manager_default_name_text, "YShell Default");

    // Applying a theme from the manager updates the Settings summary.
    let dracula_index = manager
        .theme_manager_rows
        .iter()
        .position(|row| row.id == "dracula")
        .expect("dracula row");
    runtime.select_theme_manager_row(i32::try_from(dracula_index).expect("index"));
    runtime
        .apply_theme_manager_selection()
        .expect("apply default theme");

    let closed = runtime.close_theme_manager();
    assert!(!closed.theme_manager_visible);
    assert!(!closed.theme_editor_open);
    assert!(closed.settings_appearance_visible);
    assert_eq!(
        closed.settings_appearance_color_scheme_name_text, "Dracula",
        "Settings keeps showing the current global theme"
    );
    assert!(
        closed.settings_appearance_preview.is_some(),
        "Settings must keep its preview after the manager closes"
    );
    let settings_closed = runtime.close_settings_appearance();
    assert!(!settings_closed.settings_appearance_visible);
    assert!(settings_closed.settings_appearance_preview.is_none());
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

/// Visible text of a preview line (wide continuations carry only a filler).
fn preview_line_text(line: &[TerminalCell]) -> String {
    line.iter()
        .filter(|cell| !cell.wide_continuation)
        .map(|cell| cell.grapheme.as_str())
        .collect()
}

fn preview_texts(lines: &[Vec<TerminalCell>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| preview_line_text(line.as_slice()))
        .collect()
}

#[test]
fn preview_lines_include_current_language_and_a_fixed_cjk_sample() {
    let palette = TerminalPalette::DEFAULT;
    let english = crate::runtime::theme::preview_lines(palette, "en-US");
    let chinese = crate::runtime::theme::preview_lines(palette, "zh-CN");
    let english_text = preview_texts(&english);
    let chinese_text = preview_texts(&chinese);

    // English UI keeps its English sample and still gets the CJK fallback line.
    assert!(
        english_text
            .iter()
            .any(|line| line == "Hello, world · theme preview"),
        "missing English sample: {english_text:?}"
    );
    assert!(
        english_text
            .iter()
            .any(|line| line == "CJK fallback: 你好，世界"),
        "missing CJK fallback line: {english_text:?}"
    );
    // Chinese UI gets a Chinese sample; the CJK fallback line is shared.
    assert!(
        chinese_text
            .iter()
            .any(|line| line == "你好，世界 · 终端主题预览"),
        "missing Chinese sample: {chinese_text:?}"
    );
    assert!(
        chinese_text
            .iter()
            .any(|line| line == "CJK fallback: 你好，世界"),
        "missing CJK fallback line: {chinese_text:?}"
    );
    // Both languages keep the ASCII prompt/listing samples.
    for texts in [&english_text, &chinese_text] {
        assert!(
            texts.iter().any(|line| line.contains("user@host:~$ ls --color")),
            "missing ASCII prompt: {texts:?}"
        );
    }
    assert_eq!(english.len(), chinese.len());
}

#[test]
fn preview_lines_keep_the_wide_cell_and_preview_width_conventions() {
    let palette = TerminalPalette::DEFAULT;
    let lines = crate::runtime::theme::preview_lines(palette, "zh-CN");
    let cjk = lines
        .iter()
        .find(|line| preview_line_text(line) == "CJK fallback: 你好，世界")
        .expect("CJK fallback line");

    let lead = cjk
        .iter()
        .position(|cell| cell.grapheme == "你")
        .expect("CJK lead cell");
    assert!(
        cjk[lead + 1].wide_continuation,
        "wide glyph must be followed by a continuation cell"
    );
    assert_eq!(cjk[lead + 1].grapheme, " ");
    assert_eq!(
        preview_line_text(cjk),
        "CJK fallback: 你好，世界",
        "continuation cells must not duplicate the glyph"
    );

    // The sample must stay within the preview surface at the default font/DPI:
    // 29 cells × 10 physical px = 290 px, well inside the 420 px bitmap area
    // (`SETTINGS_PREVIEW_AREA_PX`); larger fonts shrink the rendered grid.
    let widest = lines.iter().map(Vec::len).max().unwrap_or(0);
    assert!(
        widest <= 29,
        "preview sample must keep the fixed preview width, got {widest} columns"
    );
}

#[test]
fn preview_lines_follow_explicit_language_values() {
    let palette = TerminalPalette::DEFAULT;
    for language in ["zh", "zh-CN", "zh-Hans", " ZH-cn "] {
        let texts = preview_texts(&crate::runtime::theme::preview_lines(palette, language));
        assert!(
            texts.iter().any(|line| line == "你好，世界 · 终端主题预览"),
            "language {language:?} must select the Chinese sample: {texts:?}"
        );
    }
    for language in ["en", "en-US", "EN-us"] {
        let texts = preview_texts(&crate::runtime::theme::preview_lines(palette, language));
        assert!(
            texts
                .iter()
                .any(|line| line == "Hello, world · theme preview"),
            "language {language:?} must select the English sample: {texts:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// TERM-2: Settings live preview vs. real terminal rendering
// ---------------------------------------------------------------------------

/// RGBA of one cell's centre pixel in a rendered frame.
fn frame_cell_pixel(frame: &yshell_terminal::TerminalFrame, column: u16, row: u16) -> [u8; 4] {
    let cell_width = frame.cell_width as usize;
    let cell_height = frame.cell_height as usize;
    let x = usize::from(column) * cell_width + cell_width / 2;
    let y = usize::from(row) * cell_height + cell_height / 2;
    let index = (y * frame.width as usize + x) * 4;
    [
        frame.rgba[index],
        frame.rgba[index + 1],
        frame.rgba[index + 2],
        frame.rgba[index + 3],
    ]
}

fn color_rgba(color: TerminalColor) -> [u8; 4] {
    [color.red, color.green, color.blue, 255]
}

fn preview_frame(runtime: &AppRuntime) -> yshell_terminal::TerminalFrame {
    runtime
        .projection()
        .settings_appearance_preview
        .expect("Settings preview frame")
}

#[test]
fn settings_preview_uses_the_window_scale_factor() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.open_settings_appearance();

    let default_frame = preview_frame(&runtime);
    let one_x = yshell_terminal::TerminalRenderer::with_scale_factor(16.0, 1.0).cell_size();
    assert_eq!(
        (default_frame.cell_width, default_frame.cell_height),
        one_x,
        "default 1.0 DPR must use the live-terminal cell metrics"
    );

    // The same preview at 125% / 150% / 200% DPI must rasterize with the same
    // physical cell metrics as the live terminal (e.g. 12x24, 15x29, 19x38 for
    // DejaVu 16px), never an upscaled 1x bitmap or a resampled image.
    for scale in [1.25_f32, 1.5, 2.0] {
        runtime.note_ui_scale_factor(scale);
        assert_eq!(runtime.ui_scale_factor(), scale);
        let scaled = preview_frame(&runtime);
        let expected =
            yshell_terminal::TerminalRenderer::with_scale_factor(16.0, scale).cell_size();
        assert_eq!(
            (scaled.cell_width, scaled.cell_height),
            expected,
            "preview cell metrics must match the live terminal at {scale}x DPI"
        );
        assert_ne!(
            (scaled.cell_width, scaled.cell_height),
            (default_frame.cell_width, default_frame.cell_height),
            "high-DPI preview must not reuse the 1x cell size (scale {scale})"
        );

        // 1:1 physical-pixel contract: the bitmap is an exact multiple of the
        // physical cell and floors into the preview area (never stretched).
        let (columns, rows) = crate::runtime::theme::preview_grid_size(
            crate::runtime::theme::SETTINGS_PREVIEW_AREA_PX,
            scale,
            expected,
        );
        assert_eq!(scaled.width, u32::from(columns) * u32::from(expected.0));
        assert_eq!(scaled.height, u32::from(rows) * u32::from(expected.1));
        assert!(
            scaled.width as f32 <= crate::runtime::theme::SETTINGS_PREVIEW_AREA_PX.0 * scale,
            "preview bitmap must fit the surface at {scale}x DPI"
        );
        assert!(
            scaled.height as f32 <= crate::runtime::theme::SETTINGS_PREVIEW_AREA_PX.1 * scale,
            "preview bitmap must fit the surface at {scale}x DPI"
        );
    }

    // Invalid DPRs fall back to 1.0 (same sanitizing as the terminal renderer).
    runtime.note_ui_scale_factor(f32::NAN);
    assert_eq!(runtime.ui_scale_factor(), 1.0);
    runtime.note_ui_scale_factor(0.0);
    assert_eq!(runtime.ui_scale_factor(), 1.0);
    runtime.note_ui_scale_factor(-2.0);
    assert_eq!(runtime.ui_scale_factor(), 1.0);
}

#[test]
fn settings_preview_does_not_clamp_the_profile_font_size() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.open_settings_appearance();
    runtime.note_ui_scale_factor(1.0);
    let area = crate::runtime::theme::SETTINGS_PREVIEW_AREA_PX;

    // 40 px used to be clamped to 20 px for the preview only.
    runtime.update_settings_appearance_font_size("40");
    let frame = preview_frame(&runtime);
    let forty = yshell_terminal::TerminalRenderer::with_font_size(40.0).cell_size();
    let twenty = yshell_terminal::TerminalRenderer::with_font_size(20.0).cell_size();
    assert_eq!((frame.cell_width, frame.cell_height), forty);
    assert!(
        frame.cell_width > twenty.0,
        "preview font size must not be clamped to 20 px"
    );

    // The fixed preview box cannot hold the full sample at 40 px: the grid is
    // reduced (fewer rows/columns) instead of the bitmap being scaled.
    let (columns, rows) = crate::runtime::theme::preview_grid_size(area, 1.0, forty);
    assert_eq!(frame.width, u32::from(columns) * u32::from(forty.0));
    assert_eq!(frame.height, u32::from(rows) * u32::from(forty.1));
    assert!(frame.width as f32 <= area.0);
    assert!(frame.height as f32 <= area.1);

    // The old 8 px preview floor is gone as well: 6 px is a valid profile size.
    runtime.update_settings_appearance_font_size("6");
    let frame = preview_frame(&runtime);
    let six = yshell_terminal::TerminalRenderer::with_font_size(6.0).cell_size();
    assert_eq!((frame.cell_width, frame.cell_height), six);
}

#[test]
fn preview_content_keeps_language_cjk_rows_and_wide_pairs_when_trimmed() {
    let palette = TerminalPalette::DEFAULT;

    // Four rows cannot hold the eight-row sample; the localized line and the
    // fixed CJK fallback line (TERM-3) must survive the row reduction.
    let fitted = crate::runtime::theme::preview_content(palette, "zh-CN", 40, 4);
    assert_eq!(fitted.lines.len(), 4);
    let texts: Vec<String> = preview_texts(&fitted.lines)
        .into_iter()
        .map(|line| line.trim_end().to_owned())
        .collect();
    assert!(
        texts.iter().any(|line| line == "你好，世界 · 终端主题预览"),
        "localized sample row must survive row reduction: {texts:?}"
    );
    assert!(
        texts.iter().any(|line| line == "CJK fallback: 你好，世界"),
        "CJK fallback row must survive row reduction: {texts:?}"
    );
    // Prompt row is kept (0,1,6,7), the selection row is not.
    assert!(fitted.cursor.is_some());
    assert!(fitted.selection.is_none());
    assert!(fitted.lines.iter().all(|line| line.len() == 40));

    // Truncation never leaves half of a wide (CJK) pair at the right edge.
    let narrow = crate::runtime::theme::preview_content(palette, "zh-CN", 15, 40);
    for line in &narrow.lines {
        assert_eq!(line.len(), 15);
        for (index, cell) in line.iter().enumerate() {
            let wide_lead = !cell.wide_continuation
                && crate::runtime::theme::preview_char_width(
                    cell.grapheme.chars().next().unwrap_or(' '),
                ) >= 2;
            if wide_lead {
                assert!(
                    line.get(index + 1)
                        .is_some_and(|next| next.wide_continuation),
                    "wide lead at column {index} lost its continuation cell"
                );
            }
        }
    }
}

#[test]
fn settings_preview_paints_cursor_selection_and_palette_colors() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.open_settings_appearance();
    runtime.note_ui_scale_factor(1.0);
    let palette = runtime.settings_appearance_palette();
    let frame = preview_frame(&runtime);
    let columns = u16::try_from(frame.width / frame.cell_width).expect("columns");
    let rows = u16::try_from(frame.height / frame.cell_height).expect("rows");
    let content =
        crate::runtime::theme::preview_content(palette, runtime.ui_language(), columns, rows);

    // Bitmap background is the terminal palette background (the surface token
    // only shows in the padding/remainder gutter).
    assert_eq!(
        frame_cell_pixel(&frame, columns - 1, rows - 1),
        color_rgba(palette.background),
        "preview cell padding must use the palette background"
    );

    // Block cursor on the prompt's trailing cell uses the palette cursor color,
    // exactly like the live terminal renderer.
    assert_eq!(content.cursor, Some((23, 1)), "cursor on the prompt row");
    let (cursor_column, cursor_row) = content.cursor.expect("cursor");
    assert_eq!(
        frame_cell_pixel(&frame, cursor_column, cursor_row),
        color_rgba(palette.cursor)
    );

    // The sample's selection row is highlighted through the renderer's normal
    // selection path (solid palette.selection behind the glyphs).
    let selection = content.selection.expect("selection");
    assert_eq!(selection.start.row, 4);
    assert_eq!(
        frame_cell_pixel(&frame, 9, selection.start.row),
        color_rgba(palette.selection),
        "selection highlight must use palette.selection"
    );

    // ANSI swatches keep their palette entries (background + foreground cell).
    assert_eq!(
        frame_cell_pixel(&frame, 5, 3),
        color_rgba(palette.ansi(0)),
        "ANSI swatch cell must use the palette ANSI entry"
    );
}

#[test]
fn settings_preview_updates_immediately_on_theme_change() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.open_settings_appearance();
    runtime.note_ui_scale_factor(1.0);

    runtime.select_settings_appearance_scheme("dracula");
    let palette = runtime.settings_appearance_palette();
    let dracula = color_scheme("dracula").expect("dracula scheme").palette;
    assert_eq!(palette.background, dracula.background);
    assert_ne!(palette.background, TerminalPalette::DEFAULT.background);

    let frame = preview_frame(&runtime);
    let columns = u16::try_from(frame.width / frame.cell_width).expect("columns");
    let rows = u16::try_from(frame.height / frame.cell_height).expect("rows");
    // The last viewport row is padding, so the whole row is palette background.
    assert_eq!(
        frame_cell_pixel(&frame, columns - 1, rows - 1),
        color_rgba(dracula.background)
    );

    // Font/fallback-font edits also re-render the preview on the same call.
    runtime.update_settings_appearance_font_size("22");
    let cell = preview_frame(&runtime);
    assert_eq!(
        (cell.cell_width, cell.cell_height),
        yshell_terminal::TerminalRenderer::with_font_size(22.0).cell_size()
    );
    runtime.update_settings_appearance_backup_font("/nonexistent/preview-font.ttf");
    assert!(
        runtime.projection().settings_appearance_preview.is_some(),
        "invalid fallback fonts must not remove the preview"
    );
}

// ---------------------------------------------------------------------------
// TERM-4: Session Editor appearance - ComboBox rows + live preview
// ---------------------------------------------------------------------------

fn session_editor_preview(runtime: &AppRuntime) -> yshell_terminal::TerminalFrame {
    runtime
        .projection()
        .session_editor_preview
        .expect("Session Editor preview frame")
}

fn editor_appearance_row<'a>(
    projection: &'a AppProjection,
    key: &str,
) -> &'a crate::runtime::TriStateFieldData {
    projection
        .editor_appearance_fields
        .iter()
        .find(|field| field.key == key)
        .unwrap_or_else(|| panic!("Session Editor appearance row `{key}`"))
}

fn folder_editor_row(
    fields: &[crate::runtime::TriStateFieldData],
    key: &str,
) -> crate::runtime::TriStateFieldData {
    fields
        .iter()
        .find(|field| field.key == key)
        .unwrap_or_else(|| panic!("Folder Editor row `{key}`"))
        .clone()
}

/// Index of one option label; the WinComboBox model is exactly `options`, so
/// this is also the index the combo reports through `selected(index)`.
fn choice_index(field: &crate::runtime::TriStateFieldData, label: &str) -> i32 {
    let index = field
        .options
        .iter()
        .position(|option| option == label)
        .unwrap_or_else(|| panic!("option `{label}` missing from {:?}", field.options));
    i32::try_from(index).expect("option index fits i32")
}

#[test]
fn session_editor_preview_only_renders_while_appearance_is_active() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.note_ui_scale_factor(1.0);

    let opened = runtime.start_new_saved_session_editor();
    assert!(opened.editor_modal_visible);
    assert_eq!(opened.editor_section_text, "general");
    assert!(
        opened.session_editor_preview.is_none(),
        "the editor preview must not render outside the Appearance section"
    );

    let appearance = runtime.select_session_editor_appearance();
    assert_eq!(appearance.editor_section_text, "appearance");
    let frame = appearance
        .session_editor_preview
        .expect("Session Editor preview on the Appearance section");

    // Same physical cell metrics as the live terminal at the same DPR + font
    // (16 px default), i.e. no preview-only clamp.
    let expected = yshell_terminal::TerminalRenderer::with_scale_factor(16.0, 1.0).cell_size();
    assert_eq!((frame.cell_width, frame.cell_height), expected);
    // 1:1 physical-pixel bitmap floored into the fixed surface area.
    let area = crate::runtime::theme::SESSION_EDITOR_PREVIEW_AREA_PX;
    let (columns, rows) = crate::runtime::theme::preview_grid_size(area, 1.0, expected);
    assert_eq!(frame.width, u32::from(columns) * u32::from(expected.0));
    assert_eq!(frame.height, u32::from(rows) * u32::from(expected.1));
    assert!(frame.width as f32 <= area.0);
    assert!(frame.height as f32 <= area.1);

    // The editor preview follows the window DPR like Settings (125% / 200%).
    for scale in [1.25_f32, 2.0] {
        runtime.note_ui_scale_factor(scale);
        let scaled = session_editor_preview(&runtime);
        let expected =
            yshell_terminal::TerminalRenderer::with_scale_factor(16.0, scale).cell_size();
        assert_eq!(
            (scaled.cell_width, scaled.cell_height),
            expected,
            "editor preview cell metrics must match the live terminal at {scale}x DPI"
        );
        let (columns, rows) = crate::runtime::theme::preview_grid_size(area, scale, expected);
        assert_eq!(scaled.width, u32::from(columns) * u32::from(expected.0));
        assert_eq!(scaled.height, u32::from(rows) * u32::from(expected.1));
        assert!(scaled.width as f32 <= area.0 * scale);
        assert!(scaled.height as f32 <= area.1 * scale);
    }
    runtime.note_ui_scale_factor(1.0);

    // Moving to another section drops the frame (no rasterization while the
    // preview is not visible).
    let general = runtime.select_session_editor_general();
    assert!(general.session_editor_preview.is_none());

    // Closing the editor drops it as well.
    runtime.select_session_editor_appearance();
    let closed = runtime.request_close_session_editor_modal();
    assert!(!closed.editor_modal_visible);
    assert!(closed.session_editor_preview.is_none());
}

#[test]
fn session_editor_choice_combo_updates_the_draft_and_preview() {
    let temp = tempdir().expect("tempdir");
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.note_ui_scale_factor(1.0);
    runtime.start_new_saved_session_editor();
    let opened = runtime.select_session_editor_appearance();

    // A new session inherits the resolved theme; the row exposes the resolved
    // display name plus a ComboBox model of every scheme.
    let scheme_row = editor_appearance_row(&opened, "appearance.color_scheme");
    assert_eq!(scheme_row.kind, "choice");
    assert_eq!(scheme_row.state, "inherit");
    assert_eq!(scheme_row.selected, 0);
    assert_eq!(scheme_row.options[0], "YShell Default");
    assert_eq!(scheme_row.value, "YShell Default");

    // Selecting an index (what WinComboBox reports) flips the field to explicit,
    // stores the scheme id in the draft and re-renders the preview immediately.
    let dracula_index = choice_index(scheme_row, "Dracula");
    let selected = runtime.select_editor_scheme_index(dracula_index);
    let row = editor_appearance_row(&selected, "appearance.color_scheme").clone();
    assert_eq!(row.state, "explicit");
    assert_eq!(row.selected, dracula_index);
    assert_eq!(row.value, "dracula");
    assert_eq!(runtime.editor.theme.theme.color_scheme.text, "dracula");
    assert!(runtime.editor.theme.theme.color_scheme.explicit);

    let dracula = color_scheme("dracula").expect("dracula palette").palette;
    let frame = session_editor_preview(&runtime);
    let columns = u16::try_from(frame.width / frame.cell_width).expect("columns");
    let rows = u16::try_from(frame.height / frame.cell_height).expect("rows");
    // The last viewport row is padding, so the whole row is palette background.
    assert_eq!(
        frame_cell_pixel(&frame, columns - 1, rows - 1),
        color_rgba(dracula.background),
        "the preview must show the selected scheme immediately"
    );

    // Font family is a choice too: picking an index stores that preset label in
    // the draft (what save resolves) and keeps the preview rendered.
    let font_row = editor_appearance_row(&selected, "appearance.font_family").clone();
    assert_eq!(font_row.kind, "choice");
    assert!(font_row.selected >= 0);
    let font_index = font_row.selected;
    let font_label = font_row.options[usize::try_from(font_index).expect("index")].clone();
    let font_selected = runtime.select_editor_font_index(font_index);
    let row = editor_appearance_row(&font_selected, "appearance.font_family").clone();
    assert_eq!(row.state, "explicit");
    assert_eq!(row.value, font_label);
    assert!(font_selected.session_editor_preview.is_some());

    // Font-size edits change the preview cell metrics on the same call; there is
    // no preview-only 8-20 clamp.
    let resized = runtime.update_editor_theme_field("appearance.font_size", "22");
    let frame = resized
        .session_editor_preview
        .expect("preview after a font-size edit");
    assert_eq!(
        (frame.cell_width, frame.cell_height),
        yshell_terminal::TerminalRenderer::with_font_size(22.0).cell_size()
    );
    let huge = runtime.update_editor_theme_field("appearance.font_size", "40");
    let frame = huge.session_editor_preview.expect("preview at 40 px");
    assert_eq!(
        (frame.cell_width, frame.cell_height),
        yshell_terminal::TerminalRenderer::with_font_size(40.0).cell_size(),
        "large sizes must not be clamped to 20 px for the preview"
    );

    // Invalid text keeps the inherited size and the preview alive (save still
    // reports the error later).
    runtime.update_editor_theme_field("appearance.font_size", "0");
    assert!(runtime.projection().session_editor_preview.is_some());
}

#[test]
fn session_editor_discard_restores_the_persisted_theme() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");
    runtime.note_ui_scale_factor(1.0);

    runtime.select_saved_session_by_id("saved-one");
    runtime
        .load_selected_saved_session_into_editor()
        .expect("load editor");
    let opened = runtime.select_session_editor_appearance();
    let baseline = editor_appearance_row(&opened, "appearance.color_scheme").clone();
    assert_eq!(baseline.state, "inherit");
    let baseline_index = baseline.selected;

    let dracula_index = choice_index(&baseline, "Dracula");
    runtime.select_editor_scheme_index(dracula_index);
    let dirty = runtime.projection();
    assert!(dirty.editor_is_dirty);
    assert!(dirty.session_editor_preview.is_some());
    assert_eq!(
        editor_appearance_row(&dirty, "appearance.color_scheme").state,
        "explicit"
    );

    // Cancel -> Discard Changes: no save happens, the preview disappears and the
    // saved session keeps its inherited theme.
    runtime.request_close_session_editor_modal();
    let closed = runtime.confirm_discard_session_editor();
    assert!(!closed.editor_modal_visible);
    assert!(closed.session_editor_preview.is_none());
    assert!(
        runtime
            .config_document
            .find_session("saved-one")
            .expect("saved session")
            .terminal
            .is_none(),
        "discard must not write the draft theme"
    );

    // Reopening renders the persisted state again.
    runtime
        .load_selected_saved_session_into_editor()
        .expect("reload editor");
    let reopened = runtime.select_session_editor_appearance();
    let scheme = editor_appearance_row(&reopened, "appearance.color_scheme");
    assert_eq!(scheme.state, "inherit");
    assert_eq!(scheme.selected, baseline_index);
    assert!(reopened.session_editor_preview.is_some());
}

#[test]
fn folder_editor_choice_rows_feed_the_combo_box() {
    let temp = tempdir().expect("tempdir");
    store_with_prod_folder(temp.path());
    let mut runtime =
        AppRuntime::new_with_keychain(temp.path().to_path_buf(), None).expect("runtime");

    let opened = runtime
        .open_folder_editor("folder-prod")
        .expect("open folder editor");
    let scheme_row = folder_editor_row(
        &opened.folder_editor_appearance_fields,
        "appearance.color_scheme",
    );
    assert_eq!(scheme_row.kind, "choice");
    assert_eq!(scheme_row.state, "explicit");
    assert_eq!(scheme_row.value, "nord");
    assert_eq!(scheme_row.selected, choice_index(&scheme_row, "Nord"));

    // Picking another option by index updates the folder draft (the same path
    // WinComboBox uses) and keeps the row's selected index in sync.
    let dracula_index = choice_index(&scheme_row, "Dracula");
    let selected = runtime.select_folder_editor_scheme_index(dracula_index);
    let row = folder_editor_row(
        &selected.folder_editor_appearance_fields,
        "appearance.color_scheme",
    );
    assert_eq!(row.selected, dracula_index);
    assert_eq!(row.value, "dracula");
    assert!(runtime.folder_editor.theme.color_scheme.explicit);

    // Logging format is the other Folder Editor choice row.
    let format_row = folder_editor_row(
        &opened.folder_editor_logging_fields,
        "logging.format",
    );
    assert_eq!(format_row.kind, "choice");
    assert_eq!(
        format_row.options,
        vec!["raw".to_owned(), "sanitized".to_owned()]
    );
    let selected = runtime.select_folder_editor_log_format_index(1);
    let format_row = folder_editor_row(
        &selected.folder_editor_logging_fields,
        "logging.format",
    );
    assert_eq!(format_row.selected, 1);
    assert_eq!(format_row.value, "sanitized");

    // Saving still works with the combo-driven draft.
    runtime.save_folder_editor().expect("save folder defaults");
    let folder = runtime
        .config_document
        .find_folder("folder-prod")
        .expect("folder");
    assert_eq!(
        folder
            .terminal
            .as_ref()
            .and_then(|terminal| terminal.color_scheme.as_deref()),
        Some("dracula")
    );
    assert_eq!(
        folder
            .logging
            .as_ref()
            .map(|logging| logging.format.as_str()),
        Some("sanitized")
    );
}
