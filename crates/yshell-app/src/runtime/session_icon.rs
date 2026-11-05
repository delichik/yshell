//! Maps persisted `SessionProfile.icon` values to Slint preset icon names.

/// Preset icon id for the session tree / tabs (`ui/components/basic/icons.slint`).
///
/// Custom image paths are not loaded in the tree yet; they fall back to `link`.
pub fn session_tree_icon_name(icon: Option<&str>) -> &'static str {
    let Some(value) = icon.map(str::trim).filter(|value| !value.is_empty()) else {
        return "terminal";
    };
    match value {
        "server" => "globe",
        "database" => "code",
        "terminal" => "terminal",
        "globe" => "globe",
        "home" => "home",
        "code" => "code",
        "folder" => "folder",
        "link" => "link",
        _ => "link",
    }
}

#[cfg(test)]
mod tests {
    use super::session_tree_icon_name;

    #[test]
    fn maps_presets_and_defaults() {
        assert_eq!(session_tree_icon_name(None), "terminal");
        assert_eq!(session_tree_icon_name(Some("")), "terminal");
        assert_eq!(session_tree_icon_name(Some("server")), "globe");
        assert_eq!(session_tree_icon_name(Some("database")), "code");
        assert_eq!(session_tree_icon_name(Some("/icons/custom.png")), "link");
    }
}
