mod commands;
mod config;
mod crypto;
mod logging;
mod platform;
mod pty;
mod ssh;
mod terminal;

use commands::{
    sessions_list, sessions_save, settings_load, settings_save, terminal_close, terminal_close_all,
    terminal_open_local, terminal_open_ssh, terminal_resize, terminal_write,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(config::ConfigStore::load_default())
        .manage(terminal::RuntimeRegistry::default())
        .invoke_handler(tauri::generate_handler![
            sessions_list,
            sessions_save,
            settings_load,
            settings_save,
            terminal_open_local,
            terminal_open_ssh,
            terminal_resize,
            terminal_write,
            terminal_close,
            terminal_close_all,
        ])
        .run(tauri::generate_context!())
        .expect("error while running YShell");
}
