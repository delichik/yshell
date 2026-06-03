//! Quick-command palette view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickCommandItem {
    pub id: String,
    pub title: String,
    pub command: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuickCommandModel {
    pub query: String,
    pub is_open: bool,
    pub commands: Vec<QuickCommandItem>,
}

impl QuickCommandModel {
    pub fn placeholder() -> Self {
        Self {
            query: String::new(),
            is_open: false,
            commands: vec![QuickCommandItem {
                id: "connect".to_owned(),
                title: "Quick connect".to_owned(),
                command: "ssh user@example.com".to_owned(),
                tags: vec!["ssh".to_owned(), "connect".to_owned()],
            }],
        }
    }
}
