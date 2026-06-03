//! Workspace tab-bar view model.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabItem {
    pub id: String,
    pub title: String,
    pub state_label: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabModel {
    pub tabs: Vec<TabItem>,
}

impl TabModel {
    pub fn placeholder() -> Self {
        Self {
            tabs: vec![TabItem {
                id: "welcome".to_owned(),
                title: "Welcome".to_owned(),
                state_label: "Disconnected".to_owned(),
                is_active: true,
            }],
        }
    }
}
