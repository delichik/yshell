//! Session-sidebar view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTreeItem {
    pub id: String,
    pub display_name: String,
    pub kind: SessionTreeItemKind,
    pub depth: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTreeItemKind {
    Folder,
    Session,
    Favorite,
    Recent,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionTreeModel {
    pub items: Vec<SessionTreeItem>,
    pub selected_id: Option<String>,
}

impl SessionTreeModel {
    pub fn placeholder() -> Self {
        Self {
            items: vec![SessionTreeItem {
                id: "welcome".to_owned(),
                display_name: "Welcome".to_owned(),
                kind: SessionTreeItemKind::Session,
                depth: 0,
            }],
            selected_id: Some("welcome".to_owned()),
        }
    }
}
