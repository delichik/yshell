//! App-shell command dispatcher placeholder.

use yshell_ui::ViewCommand;

#[derive(Debug, Default)]
pub struct AppCommandDispatcher;

impl AppCommandDispatcher {
    pub fn dispatch(&self, command: ViewCommand) {
        println!("queued UI command: {command:?}");
    }
}
