//! N5 终端主题：外观解析、Settings 外观、Folder/Session 三态覆盖；
//! 命名主题库（内置只读 / 用户可克隆）；以及 D17（只对新开终端生效）。

use tempfile::tempdir;
use yshell_config::{ConfigDocument, ConfigStore, FolderProfile, SessionProfile, TerminalProfile};
use yshell_terminal::{color_scheme, TerminalCell, TerminalColor, TerminalPalette};

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
