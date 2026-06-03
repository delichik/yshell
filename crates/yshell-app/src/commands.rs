//! App-shell command dispatcher for native UI actions.

use yshell_ui::ViewCommand;

#[derive(Debug, Default, Clone)]
pub struct AppCommandDispatcher;

impl AppCommandDispatcher {
    pub fn dispatch(&self, command: ViewCommand) -> String {
        let message = format!("queued UI command: {command:?}");
        println!("{message}");
        message
    }
}
