//! YShell application entry point.

mod app_state;
mod bootstrap;
mod commands;
mod error;

use bootstrap::bootstrap_app;
use error::AppResult;

fn main() -> AppResult<()> {
    let app = bootstrap_app()?;
    app.run()
}
