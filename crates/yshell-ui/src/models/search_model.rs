//! Search view model for terminal and remote listings.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SearchModel {
    pub query: String,
    pub scope: SearchScope,
    pub match_count: usize,
    pub current_match: Option<usize>,
    pub case_sensitive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchScope {
    #[default]
    ActiveTerminal,
    AllTabs,
    SftpListing,
}
