//! Out-of-process Shell: Slint UI attached to a Host-owned AppRuntime (D38).
//!
//! D38 multi-window: the Shell owns N Slint `MainWindow`s, each bound to a Host
//! window id. The Host owns the window model (tab ownership / per-window active
//! tab / focused window) and allocates window ids; the Shell creates one Slint
//! window per id, renders that window's projection and reports the live window
//! count (zero => Host shutdown). Auxiliary dialogs stay on the first ("dialog
//! host") window. See `docs/prototypes/process-architecture.md`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use copypasta::{ClipboardContext, ClipboardProvider};
use serde_json::{json, Value};
use slint::{ComponentHandle, SharedPixelBuffer, Timer, TimerMode};

use crate::bootstrap::apply_projection;
use crate::bootstrap::generated_ui::{MainWindow, TerminalScrollInfo};
use crate::bootstrap::{spawn_pick_folder, spawn_pick_manager_file, DialogKind};
use crate::error::{AppError, AppResult};
use crate::host::shell_ipc::{
    projection_from_json, HostShellClient, WireSelection, WireTerminalAppearance,
    WireTerminalSnapshot,
};
use crate::host::{connect_shell_client, AppRuntimeClient};
use crate::remote_dialog_wire::FileTx;
use crate::remote_dialogs::{sync_from_value, RemoteDialogHost};
use yshell_terminal::{TerminalCell, TerminalColor, TerminalRenderer, TerminalSnapshot};

/// One Shell window bound to a Host window id.
struct RemoteWindow {
    id: u64,
    window: MainWindow,
    renderer: Rc<RefCell<TerminalRenderer>>,
    /// Last rendered Host frame id. `None` forces a re-render (DPI / viewport /
    /// appearance change), even when the Host frame id itself is unchanged.
    last_frame: Rc<Cell<Option<u64>>>,
    last_viewport: Rc<Cell<(u16, u16, f32)>>,
    viewport_ticks: Rc<Cell<u32>>,
    /// Active tab of the last applied projection; a change re-projects the
    /// session's frozen terminal appearance.
    last_active_tab: RefCell<String>,
    /// Last secondary-pane (columns, rows) reported to the Host; `(0, 0)` means
    /// the workspace has no split (mirrors `WindowEntry::last_secondary_viewport`
    /// in the in-process path).
    last_secondary_viewport: Cell<(u16, u16)>,
    /// Appearance currently installed in this window's renderer. Skipping an
    /// identical re-apply keeps the glyph cache warm.
    applied_appearance: RefCell<Option<WireTerminalAppearance>>,
    /// True once a projection was applied at least once.
    projection_seen: Cell<bool>,
    /// Tab count of the last applied projection. A non-main window whose last tab
    /// was closed closes itself (N7 `requested_window_close` behaviour).
    tab_count: Cell<usize>,
}

/// Single-threaded Shell window registry (UI process only).
struct ShellWindows {
    windows: Vec<Rc<RemoteWindow>>,
    /// Most recently activated window (second-launch focus target).
    focused_window_id: u64,
    /// Window hosting the auxiliary dialogs (re-bound when that window closes).
    dialog_window_id: u64,
    /// Windows whose Host-side close already ran; dropped on the next timer tick
    /// (a Slint component must not be destroyed from inside its own callback).
    pending_close: Vec<u64>,
    dialogs: Rc<RefCell<RemoteDialogHost>>,
    /// Live window count reported to the Host (zero => Host shutdown).
    window_count: Rc<Cell<u32>>,
}

impl ShellWindows {
    fn get(&self, window_id: u64) -> Option<Rc<RemoteWindow>> {
        self.windows
            .iter()
            .find(|window| window.id == window_id)
            .cloned()
    }

    fn snapshot(&self) -> Vec<Rc<RemoteWindow>> {
        self.windows.clone()
    }

    /// Host main window = smallest registered id (see `AppRuntime::main_window_id`).
    fn main_window_id(&self) -> Option<u64> {
        self.windows.iter().map(|window| window.id).min()
    }

    fn dialog_window(&self) -> Option<Rc<RemoteWindow>> {
        self.get(self.dialog_window_id)
            .or_else(|| self.windows.iter().min_by_key(|window| window.id).cloned())
    }

    fn live_count(&self) -> u32 {
        u32::try_from(self.windows.len()).unwrap_or(u32::MAX)
    }
}

/// Run the Shell UI against an already-running Host.
pub fn run_remote_shell(config_dir: std::path::PathBuf) -> AppResult<()> {
    crate::win_chrome::select_backend()?;
    let client = Arc::new(connect_shell_client(&config_dir)?);
    client.attach(1)?;

    let rt = AppRuntimeClient::remote(Arc::clone(&client));
    let startup_projection = rt.call_value("prepare_desktop_startup_projection", json!([]))?;
    let _ = rt.call_value("prepare_desktop_startup_workspace", json!([]))?;
    let _ = rt.call_value("request_master_password_unlock", json!([]))?;

    // The Shell never constructs an AppRuntime: logging is initialised from the
    // Host projection (debug-logging flag) so the Host stays the only process
    // that loads/migrates/writes configuration.
    let debug_logging = startup_projection
        .get("settings_debug_logging_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Err(error) = crate::logging_setup::init_logging_for_shell(&config_dir, debug_logging) {
        eprintln!("yshell: shell logging initialisation failed: {error}");
    }

    let dialogs = Rc::new(RefCell::new(RemoteDialogHost::new()));
    let window_count = Rc::new(Cell::new(0u32));
    let shell = Rc::new(RefCell::new(ShellWindows {
        windows: Vec::new(),
        focused_window_id: 0,
        dialog_window_id: 0,
        pending_close: Vec::new(),
        dialogs,
        window_count,
    }));

    // Main window = Host window id 0 (registered before the Slint window exists).
    rt.call_null("register_window", json!([0u64]))?;
    rt.call_null("focus_window", json!([0u64]))?;
    create_remote_window(&shell, &client, 0, true)?;
    focus_window_by_id(&shell, &client, 0);
    publish_window_count(&shell, &client);

    let timer = start_shell_timer(&shell, &client, &config_dir);
    let _timer = timer;

    slint::run_event_loop().map_err(AppError::from_error)?;

    for entry in shell.borrow().snapshot() {
        let _ = entry.window.hide();
    }
    let _ = client.set_window_count(0);
    let _ = client.detach();
    Ok(())
}

/// Create one Slint window bound to a Host window id and register it.
///
/// `primary` is the window that hosts the auxiliary dialogs (the first window,
/// re-bound if it closes). Window ids come from the Host and are stable.
fn create_remote_window(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
    primary: bool,
) -> AppResult<Rc<RemoteWindow>> {
    let window = MainWindow::new().map_err(AppError::from_error)?;
    let renderer = Rc::new(RefCell::new(TerminalRenderer::new()));
    let last_frame = Rc::new(Cell::new(None));
    let last_viewport = Rc::new(Cell::new((0u16, 0u16, 0.0f32)));
    let viewport_ticks = Rc::new(Cell::new(0u32));
    let entry = Rc::new(RemoteWindow {
        id: window_id,
        window,
        renderer: Rc::clone(&renderer),
        last_frame: Rc::clone(&last_frame),
        last_viewport: Rc::clone(&last_viewport),
        viewport_ticks: Rc::clone(&viewport_ticks),
        last_active_tab: RefCell::new(String::new()),
        last_secondary_viewport: Cell::new((0, 0)),
        applied_appearance: RefCell::new(None),
        projection_seen: Cell::new(false),
        tab_count: Cell::new(0),
    });

    let dialogs = { Rc::clone(&shell.borrow().dialogs) };
    let file_tx = dialogs.borrow().file_tx();
    if primary {
        dialogs.borrow_mut().attach_main(&entry.window);
    }
    let initial_projection = client.get_projection(window_id).ok();
    if let Some(value) = &initial_projection {
        entry.projection_seen.set(true);
        entry.tab_count.set(tab_count_of(value));
        apply_host_projection(&entry.window, value.clone());
        // Item B: project the session's frozen appearance before the first
        // bitmap so the remote renderer matches the in-process one from frame 0.
        let _ = sync_active_tab_appearance(&entry, client, value);
        if primary {
            sync_from_value(&mut dialogs.borrow_mut(), client, value.clone());
        }
    }
    // Platform fonts + configured language baseline (same as the main window).
    crate::bootstrap::apply_fonts(&entry.window);
    crate::bootstrap::apply_language(entry.window.get_settings_language_text().as_str());

    wire_main_callbacks(
        &entry.window,
        window_id,
        Arc::clone(client),
        Rc::clone(&renderer),
        Rc::downgrade(shell),
        file_tx,
    );
    wire_window_lifecycle(&entry, shell, client);

    entry.window.show().map_err(AppError::from_error)?;
    if let Some(value) = &initial_projection {
        render_terminal_frame(&entry, client, value);
    }

    {
        let mut state = shell.borrow_mut();
        state.windows.push(Rc::clone(&entry));
        if primary {
            state.dialog_window_id = window_id;
        }
        if state.windows.len() == 1 {
            state.focused_window_id = window_id;
        }
    }
    publish_window_count(shell, client);
    Ok(entry)
}

/// Per-window close / quit / activation handlers.
fn wire_window_lifecycle(
    entry: &RemoteWindow,
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
) {
    let window_id = entry.id;
    {
        let rt = AppRuntimeClient::remote(Arc::clone(client));
        let shell_weak = Rc::downgrade(shell);
        entry.window.on_window_activated(move || {
            if let Some(shell) = shell_weak.upgrade() {
                shell.borrow_mut().focused_window_id = window_id;
                let _ = call_quiet(&rt, "focus_window", json!([window_id]));
            }
        });
    }
    {
        let shell_weak = Rc::downgrade(shell);
        let client = Arc::clone(client);
        // Registered after `create` returns? It is registered before `show()`; Slint
        // accepts close handlers on an unshown component.
        entry
            .window
            .window()
            .on_close_requested(move || match shell_weak.upgrade() {
                Some(shell) => handle_window_close(&shell, &client, window_id),
                None => slint::CloseRequestResponse::HideWindow,
            });
    }
    {
        let shell_weak = Rc::downgrade(shell);
        let client = Arc::clone(client);
        entry.window.on_quit_app(move || {
            if let Some(shell) = shell_weak.upgrade() {
                shutdown_shell(&shell, &client);
            }
        });
    }
}

/// Shared 16 ms timer: Host tick, per-window projection/terminal refresh, PTY
/// viewport sync and the live window-count heartbeat.
fn start_shell_timer(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    config_dir: &std::path::Path,
) -> Timer {
    let timer_client = Arc::clone(client);
    let timer_rt = AppRuntimeClient::remote(Arc::clone(client));
    let shell_weak = Rc::downgrade(shell);
    let timer_failures = Rc::new(Cell::new(0u32));
    let timer_recovery = Arc::new(AtomicBool::new(false));
    let timer_config_dir = config_dir.to_path_buf();
    let timer = Timer::default();
    timer.start(
        TimerMode::Repeated,
        std::time::Duration::from_millis(16),
        move || {
            let Some(shell) = shell_weak.upgrade() else {
                return;
            };
            // Drop Slint components whose Host-side close already ran.
            drain_pending_window_closes(&shell, &timer_client);
            let dialogs = { Rc::clone(&shell.borrow().dialogs) };
            dialogs.borrow_mut().poll_local(&timer_client);
            if dialogs.borrow().take_pending_exit() {
                shutdown_shell(&shell, &timer_client);
                return;
            }
            let dialog_dirty = {
                let dialogs = dialogs.borrow();
                dialogs.main_requests_dialog() || dialogs.take_needs_refresh()
            };
            match timer_client.host_tick() {
                Ok((dirty, _count, shutdown)) => {
                    // Reconnected: refresh every window (restores the status line).
                    let was_failing = timer_failures.replace(0) > 0;
                    if was_failing {
                        timer_recovery.store(false, Ordering::SeqCst);
                    }
                    if timer_client.take_open_window() {
                        // Second `yshell` launch: focus the focused window (or show
                        // the first one if every window is hidden).
                        focus_open_window(&shell, &timer_client);
                    }
                    if shutdown {
                        shutdown_shell(&shell, &timer_client);
                        return;
                    }
                    if dirty || was_failing {
                        let dialog_window_id = shell.borrow().dialog_window_id;
                        for entry in shell.borrow().snapshot() {
                            let sync_dialogs = entry.id == dialog_window_id;
                            refresh_remote_window(&shell, &timer_client, &entry, sync_dialogs);
                        }
                    } else if dialog_dirty {
                        let dialog_window = shell.borrow().dialog_window();
                        if let Some(entry) = dialog_window {
                            refresh_remote_window(&shell, &timer_client, &entry, true);
                        }
                    }
                    // N7: closing the last tab of a non-main window closes the window.
                    close_emptied_secondary_windows(&shell, &timer_client);
                    sync_all_viewports(&shell, &timer_rt);
                    let count = shell.borrow().window_count.get();
                    let _ = timer_client.heartbeat(count);
                }
                Err(error) => {
                    let failures = timer_failures.get().saturating_add(1);
                    timer_failures.set(failures);
                    if failures == 1 {
                        tracing::warn!(error = %error, "shell lost the Host bridge; retrying");
                    }
                    // The Host may have crashed. Try to bring it back in the
                    // background (once at a time) instead of freezing the UI.
                    if failures >= 30 && !timer_recovery.swap(true, Ordering::SeqCst) {
                        let config_dir = timer_config_dir.clone();
                        let recovery = Arc::clone(&timer_recovery);
                        std::thread::spawn(move || {
                            match crate::host::ensure_host_process(&config_dir) {
                                Ok(info) => tracing::info!(
                                    host = %info.host,
                                    port = info.port,
                                    "restarted Host after the Shell lost the bridge"
                                ),
                                Err(error) => {
                                    tracing::warn!(error = %error, "could not restart Host")
                                }
                            }
                            recovery.store(false, Ordering::SeqCst);
                        });
                    }
                    if failures == 32 {
                        for entry in shell.borrow().snapshot() {
                            entry
                                .window
                                .set_status_text("Host connection lost; reconnecting…".into());
                            entry
                                .window
                                .set_status_kind_text("host-connection-lost".into());
                        }
                    }
                }
            }
        },
    );
    timer
}

fn tab_count_of(projection: &Value) -> usize {
    projection
        .get("tabs")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

fn window_by_id(shell: &Rc<RefCell<ShellWindows>>, window_id: u64) -> Option<Rc<RemoteWindow>> {
    shell.borrow().get(window_id)
}

fn publish_window_count(shell: &Rc<RefCell<ShellWindows>>, client: &HostShellClient) {
    let count = {
        let state = shell.borrow();
        let count = state.live_count();
        state.window_count.set(count);
        count
    };
    let _ = client.set_window_count(count);
}

fn set_window_status(shell: &Rc<RefCell<ShellWindows>>, window_id: u64, text: &str) {
    if let Some(entry) = window_by_id(shell, window_id) {
        entry.window.set_status_text(text.into());
    }
}

/// Queue a Host-closed window for local teardown on the next timer tick.
fn queue_window_close(shell: &Rc<RefCell<ShellWindows>>, window_id: u64) {
    if let Some(entry) = window_by_id(shell, window_id) {
        let _ = entry.window.hide();
    }
    let mut state = shell.borrow_mut();
    if !state.pending_close.contains(&window_id) {
        state.pending_close.push(window_id);
    }
}

/// Remove queued windows, drop their Slint components, rebind the dialog host and
/// publish the new live window count.
fn drain_pending_window_closes(shell: &Rc<RefCell<ShellWindows>>, client: &Arc<HostShellClient>) {
    let pending = { std::mem::take(&mut shell.borrow_mut().pending_close) };
    if pending.is_empty() {
        return;
    }
    let mut removed = Vec::new();
    let mut new_dialog_window = None;
    {
        let mut state = shell.borrow_mut();
        for window_id in pending {
            if let Some(position) = state
                .windows
                .iter()
                .position(|window| window.id == window_id)
            {
                removed.push(state.windows.remove(position));
            }
            if state.dialog_window_id == window_id {
                state.dialog_window_id = state
                    .windows
                    .iter()
                    .map(|window| window.id)
                    .min()
                    .unwrap_or(0);
                new_dialog_window = state.get(state.dialog_window_id);
            }
            if state.focused_window_id == window_id {
                state.focused_window_id = state
                    .windows
                    .iter()
                    .map(|window| window.id)
                    .min()
                    .unwrap_or(0);
            }
        }
        let count = state.live_count();
        state.window_count.set(count);
    }
    for entry in &removed {
        let _ = entry.window.hide();
    }
    if let Some(entry) = new_dialog_window {
        let dialogs = { Rc::clone(&shell.borrow().dialogs) };
        dialogs.borrow_mut().attach_main(&entry.window);
        refresh_remote_window(shell, client, &entry, true);
    }
    if !removed.is_empty() {
        publish_window_count(shell, client);
    }
}

/// Fetch and apply one window's projection (plus its terminal bitmap when the
/// frame changed). `sync_dialogs` is true only for the dialog-host window.
fn refresh_remote_window(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    entry: &RemoteWindow,
    sync_dialogs: bool,
) {
    let Ok(value) = client.get_projection(entry.id) else {
        return;
    };
    entry.projection_seen.set(true);
    entry.tab_count.set(tab_count_of(&value));
    apply_host_projection(&entry.window, value.clone());
    // Item B: a tab switch can carry a different session (and therefore a
    // different frozen font/palette). Re-project the appearance before repaint.
    let _ = sync_active_tab_appearance(entry, client, &value);
    if sync_dialogs {
        let dialogs = { Rc::clone(&shell.borrow().dialogs) };
        sync_from_value(&mut dialogs.borrow_mut(), client, value.clone());
    }
    render_terminal_frame(entry, client, &value);
}

fn refresh_window_by_id(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
) {
    let Some(entry) = window_by_id(shell, window_id) else {
        return;
    };
    let dialog_window_id = shell.borrow().dialog_window_id;
    refresh_remote_window(shell, client, &entry, entry.id == dialog_window_id);
}

fn render_terminal_frame(entry: &RemoteWindow, client: &HostShellClient, projection: &Value) {
    let frame_id = projection
        .get("terminal_frame_id")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if entry.last_frame.get() == Some(frame_id) {
        return;
    }
    if let Ok(Some(snapshot)) = client.get_terminal_snapshot(entry.id, false) {
        // The Host bumps the frame id when it captures the frozen appearance, so
        // a new snapshot is also the point where a just-connected session's
        // configured fonts/palette arrive.
        if let Some(appearance) = snapshot.appearance.as_ref() {
            let _ = apply_wire_appearance(entry, appearance);
        }
        if let Some(image) = render_wire_snapshot(&entry.renderer, &snapshot) {
            entry.window.set_terminal_image(image);
        }
    }
    entry.last_frame.set(Some(frame_id));
}

/// Re-project the window's terminal appearance when its active tab changed.
///
/// The Host freezes a session's appearance at creation (D17), so an appearance
/// change can only arrive with a new/changed active session. Returns `false`
/// when the tab did not change — the common per-tick path performs no bridge
/// round trip.
fn sync_active_tab_appearance(
    entry: &RemoteWindow,
    client: &HostShellClient,
    projection: &Value,
) -> bool {
    let active_tab = projection
        .get("active_tab_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let changed = {
        let mut last = entry.last_active_tab.borrow_mut();
        if last.as_str() == active_tab {
            false
        } else {
            *last = active_tab.to_owned();
            true
        }
    };
    if !changed {
        return false;
    }
    // Always repaint on a tab change: the new session's frame id can coincide
    // with the previous one, which would otherwise leave the old bitmap.
    entry.last_frame.set(None);
    sync_remote_appearance(entry, client)
}

/// Fetch this window's active-session terminal appearance from the Host and
/// apply it to the window's private renderer (font family/size, palette,
/// cursor). Returns `true` when the renderer inputs changed.
///
/// Item B: the Shell owns the bitmap, so the Host's frozen appearance must be
/// projected over the bridge. This `UiCall` is used on window creation and tab
/// changes; per-frame updates ride on `WireTerminalSnapshot::appearance` (the
/// Host bumps the frame id when it captures the appearance).
fn sync_remote_appearance(entry: &RemoteWindow, client: &HostShellClient) -> bool {
    let value = match client.ui_call("window_terminal_appearance", json!([entry.id])) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, "could not read the remote terminal appearance");
            return false;
        }
    };
    if value.is_null() {
        return false;
    }
    let appearance: WireTerminalAppearance = match serde_json::from_value(value) {
        Ok(appearance) => appearance,
        Err(error) => {
            tracing::warn!(error = %error, "invalid remote terminal appearance payload");
            return false;
        }
    };
    apply_wire_appearance(entry, &appearance)
}

/// Install an appearance payload in the window's private renderer.
///
/// A rejected font file silently keeps the bundled faces (same degradation as
/// in-process `sync_appearance`). Returns `false` when the renderer already
/// matches, which keeps the glyph cache warm.
fn apply_wire_appearance(entry: &RemoteWindow, appearance: &WireTerminalAppearance) -> bool {
    if entry.applied_appearance.borrow().as_ref() == Some(appearance) {
        return false;
    }
    let (palette, font) = appearance.renderer_inputs();
    let rejected = {
        let mut renderer = entry.renderer.borrow_mut();
        renderer.set_font_size(appearance.font_size);
        renderer.apply_appearance(palette, font)
    };
    for face_error in rejected {
        tracing::warn!(
            font = face_error.face_name(),
            reason = face_error.reason(),
            "remote terminal font file rejected; bundled faces kept"
        );
    }
    entry
        .window
        .set_terminal_surface_background(slint::Color::from_rgb_u8(
            palette.background.red,
            palette.background.green,
            palette.background.blue,
        ));
    *entry.applied_appearance.borrow_mut() = Some(appearance.clone());
    // Cell metrics changed: re-render immediately and let the next timer tick
    // re-report the viewport/PTY grid (derived from this renderer's cells).
    // `viewport_ticks = 11` makes that tick the every-12th full sync.
    entry.last_frame.set(None);
    entry.last_viewport.set((0, 0, 0.0));
    entry.last_secondary_viewport.set((0, 0));
    entry.viewport_ticks.set(11);
    true
}

fn focus_window_by_id(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
) {
    let Some(entry) = window_by_id(shell, window_id) else {
        return;
    };
    shell.borrow_mut().focused_window_id = window_id;
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let _ = call_quiet(&rt, "focus_window", json!([window_id]));
    let _ = entry.window.show();
    crate::win_chrome::focus_window(entry.window.window());
    entry.window.window().request_redraw();
}

/// Second-instance launch: show + focus the focused window (fallback: first one).
fn focus_open_window(shell: &Rc<RefCell<ShellWindows>>, client: &Arc<HostShellClient>) {
    let (focused, fallback) = {
        let state = shell.borrow();
        (state.focused_window_id, state.main_window_id())
    };
    let target = window_by_id(shell, focused)
        .or_else(|| fallback.and_then(|window_id| window_by_id(shell, window_id)));
    match target {
        Some(entry) => {
            focus_window_by_id(shell, client, entry.id);
            refresh_remote_window(shell, client, &entry, true);
        }
        None => tracing::warn!("Host asked for a window but the Shell has none open"),
    }
}

/// Close a (non-main) window whose tabs were all moved out.
fn close_window_if_empty(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
) {
    let (main_window_id, empty, already_pending) = {
        let state = shell.borrow();
        (
            state.main_window_id(),
            state
                .get(window_id)
                .is_some_and(|entry| entry.tab_count.get() == 0),
            state.pending_close.contains(&window_id),
        )
    };
    if already_pending || !empty || main_window_id == Some(window_id) {
        return;
    }
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let _ = call_quiet(&rt, "request_close_window", json!([window_id]));
    queue_window_close(shell, window_id);
}

/// N7: closing the last tab of a window behaves like its close button. The Host
/// records `requested_window_close`; the Shell detects the emptied projection.
fn close_emptied_secondary_windows(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
) {
    let candidates: Vec<u64> = {
        let state = shell.borrow();
        let main_window_id = state.main_window_id();
        state
            .windows
            .iter()
            .filter(|entry| {
                Some(entry.id) != main_window_id
                    && entry.projection_seen.get()
                    && entry.tab_count.get() == 0
            })
            .filter(|entry| !state.pending_close.contains(&entry.id))
            .map(|entry| entry.id)
            .collect()
    };
    if candidates.is_empty() {
        return;
    }
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    for window_id in candidates {
        tracing::info!(
            window_id,
            "closing a secondary window whose last tab was closed"
        );
        let _ = call_quiet(&rt, "request_close_window", json!([window_id]));
        queue_window_close(shell, window_id);
    }
}

/// Window close button / last-tab close flow (N7 semantics).
fn handle_window_close(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
) -> slint::CloseRequestResponse {
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let value = match rt.call_value("request_close_window", json!([window_id])) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(error = %error, window_id, "request_close_window failed");
            set_window_status(
                shell,
                window_id,
                "Could not close the window while the Host is unreachable.",
            );
            return slint::CloseRequestResponse::KeepWindowShown;
        }
    };
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("close_now");
    let is_last = value
        .get("is_last")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    match action {
        "confirm_quit" => {
            // Last window: reuse the existing Quit confirmation dialog.
            let dialogs = { Rc::clone(&shell.borrow().dialogs) };
            dialogs.borrow_mut().request_quit_confirm();
            let dialog_window_id = shell.borrow().dialog_window_id;
            refresh_window_by_id(shell, client, dialog_window_id);
            slint::CloseRequestResponse::KeepWindowShown
        }
        "confirm_close" => {
            // The remote close-window confirmation dialog is bound to window 0
            // (`remote_dialogs` is out of this change's scope), so it cannot target
            // an arbitrary window id. Safe fallback: cancel the Host pending state
            // and move this window's tabs to another window before closing it, so
            // no session is silently disconnected.
            let _ = call_quiet(&rt, "cancel_close_window", json!([window_id]));
            match move_window_tabs_elsewhere(shell, client, window_id) {
                Ok(()) => {
                    let _ = call_quiet(&rt, "request_close_window", json!([window_id]));
                    queue_window_close(shell, window_id);
                    tracing::info!(
                        window_id,
                        "closed a window with active sessions by moving its tabs (remote close-confirm fallback)"
                    );
                    slint::CloseRequestResponse::HideWindow
                }
                Err(error) => {
                    set_window_status(
                        shell,
                        window_id,
                        &format!("Could not close this window: {error}"),
                    );
                    slint::CloseRequestResponse::KeepWindowShown
                }
            }
        }
        _ => {
            if is_last {
                shutdown_shell(shell, client);
            } else {
                queue_window_close(shell, window_id);
            }
            slint::CloseRequestResponse::HideWindow
        }
    }
}

/// Move every tab of `window_id` to the lowest-id other window.
fn move_window_tabs_elsewhere(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    window_id: u64,
) -> AppResult<()> {
    let target_window_id = {
        let state = shell.borrow();
        state
            .windows
            .iter()
            .map(|entry| entry.id)
            .filter(|id| *id != window_id)
            .min()
            .ok_or_else(|| AppError::new("no other window is open"))?
    };
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let tab_ids = rt
        .call_value("window_tab_ids", json!([window_id]))
        .map_err(|error| AppError::new(error.to_string()))?;
    let tab_ids = tab_ids.as_array().cloned().unwrap_or_default();
    for tab_id in tab_ids {
        if let Some(tab_id) = tab_id.as_str() {
            rt.call_value("move_tab_to_window", json!([tab_id, target_window_id]))
                .map_err(|error| AppError::new(error.to_string()))?;
        }
    }
    focus_window_by_id(shell, client, target_window_id);
    refresh_window_by_id(shell, client, target_window_id);
    Ok(())
}

/// Tab context menu: "Move Tab to New Window" (Host allocates the id).
fn move_tab_to_new_window(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    source_window_id: u64,
    tab_id: &str,
) {
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let source = call_quiet(&rt, "window_of_tab", json!([tab_id]))
        .and_then(|value| value.as_u64())
        .unwrap_or(source_window_id);
    let value = match rt.call_value("move_tab_to_new_window", json!([tab_id])) {
        Ok(value) => value,
        Err(error) => {
            set_window_status(
                shell,
                source_window_id,
                &format!("Could not move the tab to a new window: {error}"),
            );
            return;
        }
    };
    let Some(new_window_id) = value.get("window_id").and_then(Value::as_u64) else {
        set_window_status(
            shell,
            source_window_id,
            "Could not move the tab to a new window.",
        );
        return;
    };
    match create_remote_window(shell, client, new_window_id, false) {
        Ok(entry) => {
            focus_window_by_id(shell, client, entry.id);
            refresh_window_by_id(shell, client, source);
            close_window_if_empty(shell, client, source);
        }
        Err(error) => {
            // Opening the OS window failed: move the tab back and drop the Host
            // window registration so the tab is never stranded.
            let _ = call_quiet(&rt, "move_tab_to_window", json!([tab_id, source]));
            let _ = call_quiet(&rt, "close_window_tabs", json!([new_window_id]));
            refresh_window_by_id(shell, client, source);
            set_window_status(
                shell,
                source_window_id,
                &format!("Could not open a new window: {error}"),
            );
        }
    }
}

/// Tab context menu: "Move Tab to Main Window".
fn move_tab_to_main_window(
    shell: &Rc<RefCell<ShellWindows>>,
    client: &Arc<HostShellClient>,
    source_window_id: u64,
    tab_id: &str,
) {
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let host_main = call_quiet(&rt, "main_window_id", json!([])).and_then(|value| value.as_u64());
    let target_window_id = match host_main {
        Some(window_id) if window_by_id(shell, window_id).is_some() => window_id,
        _ => match shell.borrow().main_window_id() {
            Some(window_id) => window_id,
            None => return,
        },
    };
    let source = call_quiet(&rt, "window_of_tab", json!([tab_id]))
        .and_then(|value| value.as_u64())
        .unwrap_or(source_window_id);
    if source == target_window_id {
        focus_window_by_id(shell, client, target_window_id);
        return;
    }
    if let Err(error) = rt.call_value("move_tab_to_window", json!([tab_id, target_window_id])) {
        set_window_status(
            shell,
            source_window_id,
            &format!("Could not move the tab to the main window: {error}"),
        );
        return;
    }
    refresh_window_by_id(shell, client, target_window_id);
    refresh_window_by_id(shell, client, source);
    focus_window_by_id(shell, client, target_window_id);
    close_window_if_empty(shell, client, source);
}

/// Quit the whole Shell: stop logging, hide every window, report zero windows,
/// detach from the Host and end the Slint event loop.
fn shutdown_shell(shell: &Rc<RefCell<ShellWindows>>, client: &Arc<HostShellClient>) {
    let rt = AppRuntimeClient::remote(Arc::clone(client));
    let _ = call_quiet(&rt, "stop_all_session_logging", json!([]));
    for entry in shell.borrow().snapshot() {
        let _ = entry.window.hide();
    }
    shell.borrow().window_count.set(0);
    let _ = client.set_window_count(0);
    let _ = client.detach();
    let _ = slint::quit_event_loop();
}

fn sync_all_viewports(shell: &Rc<RefCell<ShellWindows>>, rt: &AppRuntimeClient) {
    let focused_window_id = shell.borrow().focused_window_id;
    for entry in shell.borrow().snapshot() {
        sync_window_viewport(&entry, rt, entry.id == focused_window_id);
        // Item C: the split pane's PTY is not covered by the active-pane sync.
        sync_window_secondary_viewport(&entry, rt);
    }
}

/// Report the secondary (split) pane viewport to the Host so that pane's PTY is
/// sized from the Shell window. Mirrors in-process
/// `sync_window_secondary_terminal_size` (windows.rs). No split = `(0, 0)`.
fn sync_window_secondary_viewport(entry: &RemoteWindow, rt: &AppRuntimeClient) {
    if !entry.window.get_workspace_split_active() {
        entry.last_secondary_viewport.set((0, 0));
        return;
    }
    let width = entry.window.get_terminal_viewport_secondary_width_px();
    let height = entry.window.get_terminal_viewport_secondary_height_px();
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let (cell_width, cell_height) = entry.renderer.borrow().cell_size();
    let scale = entry.renderer.borrow().scale_factor();
    let (columns, rows) = crate::bootstrap::terminal_size_from_viewport(
        width,
        height,
        (cell_width, cell_height),
        scale,
    );
    if (columns, rows) == entry.last_secondary_viewport.get() {
        return;
    }
    entry.last_secondary_viewport.set((columns, rows));
    let _ = call_quiet(
        rt,
        "sync_window_secondary_terminal_size",
        json!([entry.id, columns, rows]),
    );
}

/// Throttled viewport / DPI reporting for one window. The Shell owns the window,
/// the Host owns the PTY, so the grid must be pushed over the bridge.
fn sync_window_viewport(entry: &RemoteWindow, rt: &AppRuntimeClient, focused: bool) {
    let tick = entry.viewport_ticks.get().wrapping_add(1);
    entry.viewport_ticks.set(tick);
    let scale = crate::bootstrap::sanitize_scale_factor(entry.window.window().scale_factor());
    let previous = entry.last_viewport.get();
    // Every 12th tick (~192 ms) plus immediately on a DPI change.
    if !tick.is_multiple_of(12) && (scale - previous.2).abs() <= f32::EPSILON {
        return;
    }
    if (entry.renderer.borrow().scale_factor() - scale).abs() > f32::EPSILON {
        entry.renderer.borrow_mut().set_scale_factor(scale);
        entry.last_frame.set(None);
    }
    let width = entry.window.get_terminal_viewport_width_px();
    let height = entry.window.get_terminal_viewport_height_px();
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let cell_size = entry.renderer.borrow().cell_size();
    let (columns, rows) =
        crate::bootstrap::terminal_size_from_viewport(width, height, cell_size, scale);
    if (columns, rows, scale) == previous {
        return;
    }
    entry.last_viewport.set((columns, rows, scale));
    if focused {
        let _ = call_quiet(rt, "note_ui_scale_factor", json!([scale]));
        let _ = call_quiet(rt, "note_terminal_viewport_size", json!([columns, rows]));
    }
    // Per-window PTY sizing: the Host resolves this window's active session.
    let _ = call_quiet(
        rt,
        "sync_window_terminal_size",
        json!([entry.id, columns, rows]),
    );
    // Re-render at the new metrics on the next dirty tick.
    entry.last_frame.set(None);
}

fn apply_host_projection(window: &MainWindow, value: serde_json::Value) {
    match projection_from_json(value) {
        Ok(projection) => apply_projection(window, &projection),
        Err(error) => tracing::warn!(error = %error, "failed to decode host projection"),
    }
}

fn refresh_from_host(window: &MainWindow, client: &HostShellClient, window_id: u64) {
    if let Ok(value) = client.get_projection(window_id) {
        apply_host_projection(window, value);
    }
}

/// One Host `UiCall` without a projection refresh; failures are logged.
fn call_quiet(
    rt: &AppRuntimeClient,
    method: &str,
    args: serde_json::Value,
) -> Option<serde_json::Value> {
    match rt.call_value(method, args) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::warn!(method, error = %error, "remote Shell UiCall failed");
            None
        }
    }
}

/// The established remote pattern: `UiCall` then re-apply the window's projection.
fn call_and_refresh(
    window_id: u64,
    rt: &AppRuntimeClient,
    client: &Arc<HostShellClient>,
    window_weak: &slint::Weak<MainWindow>,
    method: &str,
    args: serde_json::Value,
) -> Option<serde_json::Value> {
    let value = call_quiet(rt, method, args);
    if let Some(window) = window_weak.upgrade() {
        refresh_from_host(&window, client, window_id);
    }
    value
}

fn read_clipboard(clipboard: &Rc<RefCell<Option<ClipboardContext>>>) -> Option<String> {
    clipboard
        .borrow_mut()
        .as_mut()
        .and_then(|context| context.get_contents().ok())
}

fn write_clipboard(clipboard: &Rc<RefCell<Option<ClipboardContext>>>, text: &str) {
    if let Some(context) = clipboard.borrow_mut().as_mut() {
        let _ = context.set_contents(text.to_owned());
    }
}

/// Map a logical-pixel pointer position on the terminal bitmap onto the
/// (column, viewport-row) grid the Host expects.
fn terminal_cell_at(
    window: &MainWindow,
    renderer: &Rc<RefCell<TerminalRenderer>>,
    x: f32,
    y: f32,
) -> Option<(i32, i32)> {
    let columns = window.get_terminal_viewport_columns();
    let rows = window.get_terminal_viewport_rows();
    if columns <= 0 || rows <= 0 {
        return None;
    }
    let scale = crate::bootstrap::sanitize_scale_factor(window.window().scale_factor());
    let (cell_width, cell_height) = renderer.borrow().cell_size();
    let column = ((x.max(0.0) * scale / cell_width.max(1) as f32).floor() as i32)
        .min(columns.saturating_sub(1));
    let row = ((y.max(0.0) * scale / cell_height.max(1) as f32).floor() as i32)
        .min(rows.saturating_sub(1));
    Some((column, row))
}

/// Wire MainWindow callbacks through Host UiCall (expanded dispatch surface).
fn wire_main_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: Arc<HostShellClient>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    shell: Weak<RefCell<ShellWindows>>,
    file_tx: FileTx,
) {
    let rt = AppRuntimeClient::remote(Arc::clone(&client));

    macro_rules! wire0 {
        ($on:ident, $method:literal) => {{
            let rt = rt.clone();
            let window_weak = window.as_weak();
            let client = Arc::clone(&client);
            window.$on(move || {
                let _ = rt.call_value($method, json!([]));
                if let Some(window) = window_weak.upgrade() {
                    refresh_from_host(&window, &client, window_id);
                }
            });
        }};
    }
    macro_rules! wire1 {
        ($on:ident, $method:literal) => {{
            let rt = rt.clone();
            let window_weak = window.as_weak();
            let client = Arc::clone(&client);
            window.$on(move |a0| {
                let _ = rt.call_value($method, json!([arg_json(&a0)]));
                if let Some(window) = window_weak.upgrade() {
                    refresh_from_host(&window, &client, window_id);
                }
            });
        }};
    }
    macro_rules! wire2 {
        ($on:ident, $method:literal) => {{
            let rt = rt.clone();
            let window_weak = window.as_weak();
            let client = Arc::clone(&client);
            window.$on(move |a0, a1| {
                let _ = rt.call_value($method, json!([arg_json(&a0), arg_json(&a1)]));
                if let Some(window) = window_weak.upgrade() {
                    refresh_from_host(&window, &client, window_id);
                }
            });
        }};
    }

    wire0!(on_open_quick_connect, "open_quick_connect");
    wire0!(on_dismiss_quick_connect, "dismiss_quick_connect");
    wire0!(on_toggle_quick_connect, "toggle_quick_connect");
    wire1!(on_update_quick_connect_input, "update_quick_connect_input");
    wire1!(on_submit_quick_connect, "handle_quick_connect");
    wire0!(
        on_quick_connect_clear_history,
        "quick_connect_history_clear"
    );
    wire0!(on_new_tab_default, "handle_new_tab_default");
    wire1!(on_activate_tab, "activate_tab");
    wire1!(on_close_tab, "request_close_tab");
    wire1!(on_close_tabs, "request_close_tabs");
    wire0!(on_cancel_close_tabs, "cancel_close_tabs");
    wire1!(on_reconnect_tab, "reconnect_tab_session");
    wire1!(on_disconnect_tab, "disconnect_tab_session");
    wire1!(on_terminal_scroll, "scroll_active_terminal");
    wire0!(
        on_terminal_scroll_to_bottom,
        "scroll_active_terminal_to_bottom"
    );
    wire0!(on_retry_connection_overlay, "retry_connection_overlay");
    wire0!(on_cancel_connection_overlay, "cancel_connection_overlay");
    wire1!(on_submit_password, "submit_password");
    wire0!(on_cancel_password_prompt, "cancel_password_prompt");
    wire0!(on_trust_host_key_once, "trust_host_key_once");
    wire0!(on_trust_host_key_and_save, "trust_host_key_and_save");
    wire0!(on_cancel_host_key_prompt, "cancel_host_key_prompt");
    wire0!(on_disconnect_session, "disconnect_active_session");
    wire0!(on_reconnect_session, "reconnect_active_session");
    {
        let rt = rt.clone();
        let window_weak = window.as_weak();
        let client = Arc::clone(&client);
        window.on_duplicate_active_tab(move || {
            if let Ok(value) = client.get_projection(window_id) {
                if let Some(tab_id) = value.get("active_tab_id").and_then(|v| v.as_str()) {
                    let _ = rt.call_value("duplicate_tab", json!([tab_id]));
                }
            }
            if let Some(window) = window_weak.upgrade() {
                refresh_from_host(&window, &client, window_id);
            }
        });
    }
    wire0!(on_stop_input_sync, "stop_input_sync_command");
    wire1!(on_split_workspace, "split_workspace");
    wire1!(on_focus_split_pane, "focus_split_pane");
    wire1!(on_close_split_pane, "close_split_pane");
    {
        let rt = rt.clone();
        let window_weak = window.as_weak();
        let client = Arc::clone(&client);
        window.on_set_split_ratio(move |split_id, ratio| {
            let _ = rt.call_value("set_split_ratio", json!([split_id, ratio]));
            if let Some(window) = window_weak.upgrade() {
                refresh_from_host(&window, &client, window_id);
            }
        });
    }
    wire0!(on_toggle_sftp, "toggle_sftp");
    wire0!(on_toggle_commands, "toggle_commands");
    wire0!(on_toggle_tunnels, "toggle_tunnels");
    wire0!(
        on_open_selected_saved_session,
        "open_selected_saved_session"
    );
    wire0!(
        on_open_selected_saved_session_sftp,
        "open_selected_saved_session_sftp"
    );
    wire0!(
        on_start_new_saved_session_editor,
        "start_new_saved_session_editor"
    );
    wire1!(on_toggle_saved_folder, "toggle_saved_folder");
    wire1!(on_set_confirm_close_session, "set_confirm_close_session");
    wire1!(on_set_confirm_quit_app, "set_confirm_quit_app");
    wire1!(on_set_mcp_enabled, "set_mcp_enabled");
    wire1!(
        on_set_mcp_auto_approve_control,
        "set_mcp_auto_approve_control"
    );
    wire0!(on_exit_mcp_control, "exit_mcp_control");

    // Settings / managers / editors — Host sets projection flags; RemoteDialogHost paints.
    wire0!(on_open_settings_appearance, "open_settings_appearance");
    wire0!(on_close_settings_appearance, "close_settings_appearance");
    wire0!(on_new_session, "handle_new_session");
    wire0!(
        on_update_selected_saved_session,
        "update_selected_saved_session_from_active"
    );
    wire0!(
        on_duplicate_selected_saved_session,
        "duplicate_selected_saved_session"
    );
    wire0!(
        on_delete_selected_saved_session,
        "delete_selected_saved_session"
    );
    wire1!(on_select_saved_session, "select_saved_session_by_id");
    wire1!(
        on_select_saved_session_for_menu,
        "select_saved_session_by_id"
    );
    wire1!(
        on_activate_saved_session_tree_node,
        "activate_saved_session_tree_node"
    );
    wire0!(
        on_open_selected_saved_session_in_new_tab,
        "open_selected_saved_session_in_new_tab"
    );
    wire0!(
        on_select_previous_saved_session,
        "select_previous_saved_session"
    );
    wire0!(on_select_next_saved_session, "select_next_saved_session");
    wire1!(on_update_session_search, "update_session_search");
    wire0!(on_terminal_logging_start, "terminal_logging_start");
    wire0!(on_terminal_logging_stop, "terminal_logging_stop");
    wire0!(on_logging_dialog_start, "start_logging_from_dialog");
    wire0!(on_logging_dialog_stop, "stop_logging_from_dialog");
    wire0!(on_logging_dialog_cancel, "close_logging_dialog");
    wire2!(on_rename_saved_session, "rename_saved_session");
    wire1!(
        on_start_new_saved_session_editor_in_folder,
        "start_new_saved_session_editor_in_folder"
    );
    wire0!(on_save_session, "save_active_session");
    wire0!(on_refresh_saved_sessions, "refresh_saved_sessions");
    wire0!(
        on_load_selected_saved_session_into_editor,
        "load_selected_saved_session_into_editor"
    );
    wire0!(on_edit_folder_defaults, "edit_folder_defaults");
    wire0!(on_open_private_keys_manager, "open_private_keys_manager");
    wire0!(on_close_private_keys_manager, "close_private_keys_manager");
    wire0!(on_open_host_keys_manager, "open_host_keys_manager");
    wire0!(on_close_host_keys_manager, "close_host_keys_manager");
    wire0!(on_open_proxies_manager, "open_proxies_manager");
    wire0!(on_close_proxies_manager, "close_proxies_manager");
    wire0!(on_open_theme_manager, "open_theme_manager");
    wire0!(on_close_theme_manager, "close_theme_manager");
    wire1!(on_toggle_panel, "toggle_panel_visible");
    wire1!(on_panel_side_expand, "expand_panel_side");
    wire1!(on_panel_side_toggle, "toggle_panel_side_expanded");
    wire1!(on_confirm_close_tabs, "confirm_close_tabs_with");

    // --- Expanded surface (D38 gap closing) ---------------------------------
    // Groups mirror the in-process wiring in `bootstrap::wire_callbacks`; each
    // callback calls the Host `UiCall` method and re-applies the projection.
    let clipboard = Rc::new(RefCell::new(ClipboardContext::new().ok()));
    wire_terminal_callbacks(window, window_id, &client, &rt, &renderer, &clipboard);
    wire_sftp_callbacks(window, window_id, &client, &rt, &clipboard, &file_tx);
    wire_local_transfer_callbacks(window, window_id, &client, &rt);
    wire_shell_window_callbacks(window, window_id, &client, &rt, &clipboard, &shell);
    // Item D: native rfd pickers for the MainWindow Browse buttons (results are
    // drained by `RemoteDialogHost::poll_local` and applied with the same
    // Host `apply_file_dialog_picked` call the text inputs use).
    wire_native_picker_callbacks(window, window_id, &client, file_tx);
}

macro_rules! wire_call0 {
    ($window_id:expr, $window:expr, $rt:expr, $client:expr, $on:ident, $method:literal) => {{
        let rt = $rt.clone();
        let client = Arc::clone($client);
        let weak = $window.as_weak();
        $window.$on(move || {
            call_and_refresh($window_id, &rt, &client, &weak, $method, json!([]));
        });
    }};
}

macro_rules! wire_call1 {
    ($window_id:expr, $window:expr, $rt:expr, $client:expr, $on:ident, $method:literal) => {{
        let rt = $rt.clone();
        let client = Arc::clone($client);
        let weak = $window.as_weak();
        $window.$on(move |a0| {
            call_and_refresh(
                $window_id,
                &rt,
                &client,
                &weak,
                $method,
                json!([arg_json(&a0)]),
            );
        });
    }};
}

macro_rules! wire_call2 {
    ($window_id:expr, $window:expr, $rt:expr, $client:expr, $on:ident, $method:literal) => {{
        let rt = $rt.clone();
        let client = Arc::clone($client);
        let weak = $window.as_weak();
        $window.$on(move |a0, a1| {
            call_and_refresh(
                $window_id,
                &rt,
                &client,
                &weak,
                $method,
                json!([arg_json(&a0), arg_json(&a1)]),
            );
        });
    }};
}

macro_rules! wire_call3 {
    ($window_id:expr, $window:expr, $rt:expr, $client:expr, $on:ident, $method:literal) => {{
        let rt = $rt.clone();
        let client = Arc::clone($client);
        let weak = $window.as_weak();
        $window.$on(move |a0, a1, a2| {
            call_and_refresh(
                $window_id,
                &rt,
                &client,
                &weak,
                $method,
                json!([arg_json(&a0), arg_json(&a1), arg_json(&a2)]),
            );
        });
    }};
}

macro_rules! wire_call4 {
    ($window_id:expr, $window:expr, $rt:expr, $client:expr, $on:ident, $method:literal) => {{
        let rt = $rt.clone();
        let client = Arc::clone($client);
        let weak = $window.as_weak();
        $window.$on(move |a0, a1, a2, a3| {
            call_and_refresh(
                $window_id,
                &rt,
                &client,
                &weak,
                $method,
                json!([arg_json(&a0), arg_json(&a1), arg_json(&a2), arg_json(&a3)]),
            );
        });
    }};
}

/// Group 1: terminal input, selection, clipboard, search and scroll.
fn wire_terminal_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: &Arc<HostShellClient>,
    rt: &AppRuntimeClient,
    renderer: &Rc<RefCell<TerminalRenderer>>,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
) {
    // Copy entries: run the Host copy, then mirror the copied text into the
    // Shell's OS clipboard.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_copy_terminal_selection(move || {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "copy_active_terminal_selection",
                json!([]),
            );
            if let Some(text) = call_quiet(&rt, "terminal_clipboard_text", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                if !text.is_empty() {
                    write_clipboard(&clipboard, &text);
                }
            }
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_copy_terminal_current_line(move || {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "copy_active_terminal_current_line",
                json!([]),
            );
            if let Some(text) = call_quiet(&rt, "terminal_clipboard_text", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                if !text.is_empty() {
                    write_clipboard(&clipboard, &text);
                }
            }
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_copy_terminal_visible(move || {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "copy_active_terminal_visible_text",
                json!([]),
            );
            if let Some(text) = call_quiet(&rt, "terminal_clipboard_text", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                if !text.is_empty() {
                    write_clipboard(&clipboard, &text);
                }
            }
        });
    }
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_terminal_select_all,
        "select_all_active_terminal"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_terminal_select_screen,
        "select_screen_active_terminal"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_clear_terminal,
        "clear_active_terminal"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_clear_terminal_scrollback,
        "clear_active_terminal_scrollback"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_paste_cross_pane,
        "paste_cross_pane"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_find_terminal_text,
        "find_in_active_terminal"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_find_terminal_next,
        "select_next_terminal_match"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_find_terminal_prev,
        "select_previous_terminal_match"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_set_terminal_search_regex,
        "set_terminal_search_regex"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_set_terminal_search_across,
        "set_terminal_search_across"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_activate_terminal_find_across_match,
        "activate_terminal_find_across_match"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_send_key_input_to,
        "start_input_sync_mode"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_toggle_tab_receive_key_input,
        "toggle_tab_receives_key_input"
    );

    // Paste from the OS clipboard (falling back to the Host clipboard buffer).
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_paste_terminal_clipboard(move || match read_clipboard(&clipboard) {
            Some(text) => {
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "paste_text_into_terminal",
                    json!([text]),
                );
            }
            None => {
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "paste_terminal_clipboard",
                    json!([]),
                );
            }
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_paste_terminal_bracketed(move || match read_clipboard(&clipboard) {
            Some(text) => {
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "paste_text_into_terminal_bracketed",
                    json!([text]),
                );
            }
            None => {
                let text = call_quiet(&rt, "terminal_clipboard_text", json!([]))
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default();
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "paste_text_into_terminal_bracketed",
                    json!([text]),
                );
            }
        });
    }

    // Pointer selection: Slint reports logical pixels; the Host maps the
    // viewport row into the scrollback-absolute row.
    {
        let suppress_selection_end = Rc::new(Cell::new(false));
        let make = |window: &MainWindow,
                    rt: &AppRuntimeClient,
                    client: &Arc<HostShellClient>,
                    renderer: &Rc<RefCell<TerminalRenderer>>,
                    method: &'static str| {
            let rt = rt.clone();
            let client = Arc::clone(client);
            let weak = window.as_weak();
            let renderer = Rc::clone(renderer);
            move |x: f32, y: f32| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let Some((column, row)) = terminal_cell_at(&window, &renderer, x, y) else {
                    return;
                };
                call_and_refresh(window_id, &rt, &client, &weak, method, json!([column, row]));
            }
        };
        window.on_terminal_selection_begin(make(
            window,
            rt,
            client,
            renderer,
            "begin_active_terminal_selection_at",
        ));
        window.on_terminal_selection_update(make(
            window,
            rt,
            client,
            renderer,
            "update_active_terminal_selection_at",
        ));
        {
            let rt = rt.clone();
            let client = Arc::clone(client);
            let weak = window.as_weak();
            let renderer = Rc::clone(renderer);
            let clipboard = Rc::clone(clipboard);
            let suppress = Rc::clone(&suppress_selection_end);
            window.on_terminal_selection_end(move |x, y| {
                if suppress.replace(false) {
                    return;
                }
                let Some(window) = weak.upgrade() else {
                    return;
                };
                if let Some((column, row)) = terminal_cell_at(&window, &renderer, x, y) {
                    let _ = call_quiet(
                        &rt,
                        "update_active_terminal_selection_at",
                        json!([column, row]),
                    );
                }
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "finish_active_terminal_selection",
                    json!([]),
                );
                if let Some(text) = call_quiet(&rt, "auto_copy_terminal_selection", json!([]))
                    .and_then(|value| value.as_str().map(str::to_owned))
                {
                    write_clipboard(&clipboard, &text);
                }
            });
        }
        {
            let rt = rt.clone();
            let client = Arc::clone(client);
            let weak = window.as_weak();
            let renderer = Rc::clone(renderer);
            let suppress = Rc::clone(&suppress_selection_end);
            window.on_terminal_select_word_at(move |x, y| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let Some((column, row)) = terminal_cell_at(&window, &renderer, x, y) else {
                    return;
                };
                // The release event that follows a double click must not
                // override the word selection with an empty drag range.
                suppress.set(true);
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "select_word_in_active_terminal_at",
                    json!([column, row]),
                );
            });
        }
    }

    // Keyboard: clipboard shortcuts stay Shell-local (the OS clipboard lives
    // here); every other key is VT-encoded by the Host so Ctrl/Alt/Meta
    // modifiers survive.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_terminal_key(move |text, ctrl, alt, shift, meta| {
            let lowered = text.to_lowercase();
            let selection_active = weak
                .upgrade()
                .map(|window| window.get_terminal_selection_active())
                .unwrap_or(false);
            if (ctrl && shift && lowered == "c")
                || (ctrl && !shift && (lowered == "c" || text == "\u{3}") && selection_active)
            {
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "copy_active_terminal_selection",
                    json!([]),
                );
                if let Some(copied) = call_quiet(&rt, "terminal_clipboard_text", json!([]))
                    .and_then(|value| value.as_str().map(str::to_owned))
                {
                    if !copied.is_empty() {
                        write_clipboard(&clipboard, &copied);
                    }
                }
                return;
            }
            if (ctrl && lowered == "v") || (shift && text == "\u{F727}") {
                match read_clipboard(&clipboard) {
                    Some(text) => {
                        call_and_refresh(
                            window_id,
                            &rt,
                            &client,
                            &weak,
                            "paste_text_into_terminal",
                            json!([text]),
                        );
                    }
                    None => {
                        call_and_refresh(
                            window_id,
                            &rt,
                            &client,
                            &weak,
                            "paste_terminal_clipboard",
                            json!([]),
                        );
                    }
                }
                return;
            }
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "send_active_terminal_key",
                json!([text.as_str(), ctrl, alt, shift, meta]),
            );
        });
    }
    // Typed line input (`Send Input to…` / demo).
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_send_terminal_input(move |input| {
            let text = input.to_string();
            if text.is_empty() {
                return;
            }
            let payload = if text.ends_with('\n') {
                text
            } else {
                format!("{text}\n")
            };
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "send_active_terminal_input",
                json!([payload]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_send_demo_input(move || {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "send_active_terminal_input",
                json!(["pwd\n"]),
            );
        });
    }
    // Scrollbar drag: the terminal view drives a global singleton, not a
    // MainWindow callback.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window
            .global::<TerminalScrollInfo>()
            .on_scroll_to_line(move |line| {
                call_and_refresh(
                    window_id,
                    &rt,
                    &client,
                    &weak,
                    "scroll_active_terminal_to_line",
                    json!([line]),
                );
            });
    }
}

/// Group 2: SFTP pane, operations and the context menu.
fn wire_sftp_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: &Arc<HostShellClient>,
    rt: &AppRuntimeClient,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
    file_tx: &FileTx,
) {
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_open_sftp_path,
        "open_sftp_path"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_open_sftp_crumb,
        "open_sftp_crumb"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_open_sftp_parent,
        "open_sftp_parent"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_refresh_sftp,
        "refresh_active_sftp_listing"
    );
    wire_call1!(window_id, window, rt, client, on_sort_sftp, "sort_sftp_by");
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_toggle_sftp_hidden,
        "toggle_sftp_hidden_files"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_sftp_column_ratio_changed,
        "set_sftp_column_ratio"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_select_sftp_row,
        "select_sftp_row"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_select_sftp_entry,
        "select_sftp_entry"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_activate_sftp_entry,
        "activate_sftp_entry"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_activate_sftp_row,
        "activate_sftp_row"
    );
    // Operation dialogs: four form fields, committed by the Host.
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_upload_sftp,
        "upload_sftp_file"
    );
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_download_sftp,
        "download_sftp_file"
    );
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_create_sftp_directory,
        "create_sftp_directory"
    );
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_rename_sftp_path,
        "rename_sftp_path"
    );
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_chmod_sftp_path,
        "chmod_sftp_path"
    );
    wire_call4!(
        window_id,
        window,
        rt,
        client,
        on_start_sftp_remote_edit,
        "start_sftp_remote_edit"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_save_sftp_remote_edit,
        "save_sftp_remote_edit"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_cancel_sftp_remote_edit,
        "cancel_sftp_remote_edit"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_create_sftp_folder,
        "create_sftp_folder_named"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_rename_sftp_selected,
        "rename_sftp_selected"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_delete_sftp_selected,
        "delete_sftp_selected"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_delete_sftp_selection,
        "delete_sftp_selection"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_chmod_sftp_selected,
        "chmod_sftp_selected"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_chmod_sftp_selection,
        "chmod_sftp_selection"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_upload_sftp_into_current,
        "upload_sftp_into_current"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_download_sftp_selected,
        "download_sftp_selected"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_edit_sftp_selected,
        "edit_sftp_selected"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_switch_active_tab_to_sftp,
        "switch_active_tab_to_sftp"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_switch_tab_to_sftp,
        "switch_tab_to_sftp"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_switch_tab_to_terminal,
        "switch_tab_to_terminal"
    );
    // Explicit-directory variants append the default "ask" conflict policy.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_upload_local_path_to_remote(move |path| {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "upload_local_path_to_remote",
                json!([path.as_str(), "ask"]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_download_sftp_selection_to(move |path| {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "download_sftp_selection_to",
                json!([path.as_str(), "ask"]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_copy_sftp_selection_to(move |path| {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "copy_sftp_selection_to",
                json!([path.as_str(), "ask"]),
            );
        });
    }
    // Context menu: native-picker entries (upload / download-to / upload-here)
    // open rfd in the Shell when available and fall back to the in-window path
    // dialog otherwise; the remaining entries are one Host call. Mirrors
    // `bootstrap::dispatch_sftp_menu_action`.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        let file_tx = file_tx.clone();
        window.on_sftp_menu_action(move |action| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let local_scope = window.get_sftp_menu_target_kind_text().starts_with("local");
            let action = action.as_str();
            if action.is_empty() {
                return;
            }
            match action {
                "download-to" => {
                    let local_dir = window.get_local_path_text().to_string();
                    if native_file_dialogs_available() {
                        spawn_pick_folder(
                            file_tx.clone(),
                            DialogKind::SftpDownloadFolder,
                            "Choose download folder",
                            &local_dir,
                        );
                    } else {
                        // No display server: keep the in-window path input.
                        window.set_sftp_dialog_local_text(local_dir.into());
                        window.set_sftp_dialog_kind("download".into());
                    }
                }
                "copy-to" => {
                    window.set_sftp_dialog_local_text(window.get_sftp_path_text());
                    window.set_sftp_dialog_kind("copy-to".into());
                }
                "delete" | "delete-selection" => {
                    window.set_sftp_dialog_kind("delete".into());
                }
                "new-folder" => {
                    window.set_sftp_remote_inline_new_folder_text("".into());
                    window.set_sftp_remote_inline_new_folder_visible(true);
                    window.set_sftp_remote_inline_rename_index(-1);
                }
                "rename" => {
                    window.set_sftp_remote_inline_rename_text(window.get_sftp_selected_name_text());
                    window.set_sftp_remote_inline_rename_index(window.get_sftp_selected_index());
                    window.set_sftp_remote_inline_new_folder_visible(false);
                }
                "download" | "download-selection" => {
                    let local_dir = window.get_local_path_text().to_string();
                    call_and_refresh(
                        window_id,
                        &rt,
                        &client,
                        &weak,
                        "download_sftp_selection_to",
                        json!([local_dir, "ask"]),
                    );
                }
                "upload" | "upload-selection" => {
                    if local_scope || action == "upload-selection" {
                        call_and_refresh(
                            window_id,
                            &rt,
                            &client,
                            &weak,
                            "upload_local_selection_to_remote",
                            json!(["ask"]),
                        );
                    } else if native_file_dialogs_available() {
                        spawn_pick_manager_file(
                            file_tx.clone(),
                            DialogKind::SftpUploadFile,
                            "Upload to remote",
                        );
                    } else {
                        window.set_sftp_dialog_local_text("".into());
                        window.set_sftp_dialog_kind("upload".into());
                    }
                }
                "upload-here" => {
                    if local_scope {
                        call_and_refresh(
                            window_id,
                            &rt,
                            &client,
                            &weak,
                            "upload_local_selection_to_remote",
                            json!(["ask"]),
                        );
                    } else {
                        let _ = call_quiet(&rt, "sftp_upload_dir_override", json!([]));
                        if native_file_dialogs_available() {
                            spawn_pick_manager_file(
                                file_tx.clone(),
                                DialogKind::SftpUploadFile,
                                "Upload into the selected remote folder",
                            );
                        } else {
                            window.set_sftp_dialog_local_text("".into());
                            window.set_sftp_dialog_kind("upload".into());
                        }
                    }
                }
                "copy-path"
                | "copy-local-path"
                | "copy-local-path-current"
                | "copy-path-current" => {
                    let (method, kind, message) = match action {
                        "copy-path" => (
                            "sftp_copy_paths_text",
                            "sftp-copied-paths",
                            "Copied remote path(s).",
                        ),
                        "copy-local-path" => (
                            "local_copy_paths_text",
                            "local-copied-paths",
                            "Copied local path(s).",
                        ),
                        "copy-local-path-current" => (
                            "local_current_path_text",
                            "local-copied-path",
                            "Copied the local path.",
                        ),
                        _ => (
                            "sftp_current_path_text",
                            "sftp-copied-path",
                            "Copied the remote path.",
                        ),
                    };
                    let text = call_quiet(&rt, method, json!([]))
                        .and_then(|value| value.as_str().map(str::to_owned))
                        .unwrap_or_default();
                    if !text.is_empty() {
                        write_clipboard(&clipboard, &text);
                    }
                    let count = if text.is_empty() {
                        0
                    } else {
                        text.lines().count()
                    };
                    let _ = call_quiet(
                        &rt,
                        "set_status_kind",
                        json!([kind, message, count.to_string(), ""]),
                    );
                    refresh_from_host(&window, &client, window_id);
                }
                other => {
                    call_and_refresh(
                        window_id,
                        &rt,
                        &client,
                        &weak,
                        "sftp_menu_action",
                        json!([other, local_scope]),
                    );
                }
            }
        });
    }
}

/// Whether a native file picker can be opened from the Shell.
///
/// Windows always has the native dialog; elsewhere rfd needs X11/Wayland.
/// This mirrors the guards inside `bootstrap::spawn_pick_manager_*` helpers,
/// which call `bootstrap::file_dialogs_available()` for the display probe.
fn native_file_dialogs_available() -> bool {
    cfg!(windows) || crate::bootstrap::file_dialogs_available()
}

/// Item D: MainWindow Browse buttons that the Shell can satisfy with rfd.
///
/// The picked path travels back over the dialog host's file channel and is
/// applied with the same Host `apply_file_dialog_picked` UiCall the in-window
/// path text input uses (see `RemoteDialogHost::poll_local`). When no native
/// dialog is available the callbacks keep the text-input fallback (status
/// message only), matching the in-process availability guards.
fn wire_native_picker_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: &Arc<HostShellClient>,
    file_tx: FileTx,
) {
    {
        let tx = file_tx.clone();
        window.on_browse_private_key_file(move || {
            spawn_pick_manager_file(
                tx.clone(),
                DialogKind::PrivateKeyImport,
                "Select an OpenSSH private key",
            );
        });
    }
    {
        let tx = file_tx.clone();
        window.on_browse_host_keys_import(move || {
            spawn_pick_manager_file(tx.clone(), DialogKind::HostKeysImport, "Import host keys");
        });
    }
    {
        let tx = file_tx.clone();
        window.on_browse_auth_prompt_key_file(move || {
            spawn_pick_manager_file(
                tx.clone(),
                DialogKind::AuthKeyFile,
                "Select a private key file",
            );
        });
    }
    // Logging folder: needs the current directory as the dialog start dir and a
    // status fallback when the platform picker is unavailable.
    {
        let rt = AppRuntimeClient::remote(Arc::clone(client));
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let tx = file_tx.clone();
        window.on_logging_dialog_browse(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !native_file_dialogs_available() {
                let _ = rt.call_value(
                    "set_logging_dialog_path_error",
                    json!([
                        "The system folder picker is unavailable here; type the folder path directly."
                    ]),
                );
                refresh_from_host(&window, &client, window_id);
                return;
            }
            let start = window.get_logging_dialog_directory_text().to_string();
            spawn_pick_folder(
                tx.clone(),
                DialogKind::LoggingDirectory,
                "Choose the log folder",
                &start,
            );
        });
    }
    // Terminal appearance font files (Settings appearance draft lives on the
    // Host until Save; a rejected file degrades to the bundled faces).
    {
        let rt = AppRuntimeClient::remote(Arc::clone(client));
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let tx = file_tx.clone();
        window.on_browse_settings_appearance_font_file(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !native_file_dialogs_available() {
                let _ = rt.call_value(
                    "set_status_kind",
                    json!([
                        "appearance-font-picker-unavailable",
                        "No display server was found; type the font file path instead.",
                        "",
                        ""
                    ]),
                );
                refresh_from_host(&window, &client, window_id);
                return;
            }
            spawn_pick_manager_file(
                tx.clone(),
                DialogKind::TerminalFontPrimary,
                "Select a terminal font file",
            );
        });
    }
    {
        let rt = AppRuntimeClient::remote(Arc::clone(client));
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_browse_settings_appearance_fallback_font(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !native_file_dialogs_available() {
                let _ = rt.call_value(
                    "set_status_kind",
                    json!([
                        "appearance-fallback-font-picker-unavailable",
                        "No display server was found; type the fallback font file path instead.",
                        "",
                        ""
                    ]),
                );
                refresh_from_host(&window, &client, window_id);
                return;
            }
            spawn_pick_manager_file(
                file_tx.clone(),
                DialogKind::TerminalFontBackup,
                "Select a fallback font file",
            );
        });
    }
}

/// In-process `data-transfer` payload for a pane drag (process-local user data;
/// the paths are resolved from the Host when the drag starts).
#[derive(Clone)]
struct SftpDragPayload {
    side: String,
    paths: Vec<String>,
    ctrl: bool,
    shift: bool,
}

fn sftp_drag_payload(data: &slint::DataTransfer) -> Option<SftpDragPayload> {
    data.user_data()
        .and_then(|value| value.downcast::<SftpDragPayload>().ok())
        .map(|value| (*value).clone())
}

fn build_drag_transfer(rt: &AppRuntimeClient, side: &str) -> slint::DataTransfer {
    let value = call_quiet(rt, "sftp_drag_payload", json!([side]));
    let payload = SftpDragPayload {
        side: side.to_owned(),
        paths: value
            .as_ref()
            .and_then(|value| value.get("paths"))
            .and_then(|paths| paths.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        ctrl: value
            .as_ref()
            .and_then(|value| value.get("ctrl"))
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false),
        shift: value
            .as_ref()
            .and_then(|value| value.get("shift"))
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false),
    };
    let mut transfer = slint::DataTransfer::default();
    transfer.set_user_data(Rc::new(payload));
    transfer
}

fn external_drop_paths(data: &slint::DataTransfer) -> Vec<String> {
    data.file_paths()
        .map(|paths| paths.map(|path| path.display().to_string()).collect())
        .unwrap_or_default()
}

#[derive(Clone)]
struct TabDragPayload {
    tab_id: String,
}

fn tab_drag_payload(data: &slint::DataTransfer) -> Option<String> {
    data.user_data()
        .and_then(|value| value.downcast::<TabDragPayload>().ok())
        .map(|value| value.tab_id.clone())
}

/// Group 3: local pane, transfer drawer, drag/drop and cross-pane clipboard.
fn wire_local_transfer_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: &Arc<HostShellClient>,
    rt: &AppRuntimeClient,
) {
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_refresh,
        "refresh_local_pane"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_up,
        "open_local_parent"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_home,
        "open_local_home"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_local_open_path,
        "open_local_path"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_local_sort_by,
        "sort_local_by"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_toggle_hidden,
        "toggle_local_hidden"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_toggle_collapsed,
        "toggle_local_collapsed"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_local_retry,
        "retry_local_pane"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_local_select_row,
        "select_local_row"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_local_activate_row,
        "activate_local_row"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_delete_local_selection,
        "delete_local_selection"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_copy_file_selection,
        "copy_selection_to_clipboard"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_cut_file_selection,
        "cut_selection_to_clipboard"
    );
    // Transfers drawer.
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_transfer_toggle_expanded,
        "toggle_transfer_drawer"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_transfer_open_sftp_drawer,
        "open_transfer_drawer"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_transfer_pause,
        "pause_sftp_transfer"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_transfer_resume,
        "resume_sftp_transfer"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_transfer_retry,
        "retry_sftp_transfer"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_transfer_cancel,
        "cancel_sftp_transfer"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_transfer_remove,
        "remove_sftp_transfer"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_transfer_clear_completed,
        "clear_completed_sftp_transfers"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_transfer_pause_all,
        "pause_all_sftp_transfers"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_transfer_resume_all,
        "resume_all_sftp_transfers"
    );
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_start_tunnel,
        "start_tunnel"
    );
    wire_call1!(window_id, window, rt, client, on_stop_tunnel, "stop_tunnel");
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_add_tunnel_from_dock_panel,
        "add_tunnel_from_dock_panel"
    );
    // Drag sources.
    {
        let local_rt = rt.clone();
        window.on_drag_local_transfer(move |_key: slint::SharedString| {
            build_drag_transfer(&local_rt, "local")
        });
        let remote_rt = rt.clone();
        window.on_drag_remote_transfer(move |_key: slint::SharedString| {
            build_drag_transfer(&remote_rt, "remote")
        });
        window.on_drag_tab_transfer(move |tab_id: slint::SharedString| {
            let mut transfer = slint::DataTransfer::default();
            transfer.set_user_data(Rc::new(TabDragPayload {
                tab_id: tab_id.to_string(),
            }));
            transfer
        });
    }
    // Drop targets.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_drop_on_local(move |data, _action| {
            let payload = sftp_drag_payload(&data);
            let side = payload.as_ref().map(|payload| payload.side.clone());
            let ctrl = payload
                .as_ref()
                .map(|payload| payload.ctrl)
                .unwrap_or(false);
            let shift = payload
                .as_ref()
                .map(|payload| payload.shift)
                .unwrap_or(false);
            let paths = payload
                .map(|payload| payload.paths)
                .unwrap_or_else(|| external_drop_paths(&data));
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "drop_on_local",
                json!([side, paths, ctrl, shift]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_drop_on_remote_row(move |index, data, _action| {
            let payload = sftp_drag_payload(&data);
            let side = payload.as_ref().map(|payload| payload.side.clone());
            let ctrl = payload
                .as_ref()
                .map(|payload| payload.ctrl)
                .unwrap_or(false);
            let shift = payload
                .as_ref()
                .map(|payload| payload.shift)
                .unwrap_or(false);
            let paths = payload
                .map(|payload| payload.paths)
                .unwrap_or_else(|| external_drop_paths(&data));
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "drop_on_remote",
                json!([side, index, paths, ctrl, shift]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_drop_on_remote_blank(move |data, _action| {
            let payload = sftp_drag_payload(&data);
            let side = payload.as_ref().map(|payload| payload.side.clone());
            let ctrl = payload
                .as_ref()
                .map(|payload| payload.ctrl)
                .unwrap_or(false);
            let shift = payload
                .as_ref()
                .map(|payload| payload.shift)
                .unwrap_or(false);
            let paths = payload
                .map(|payload| payload.paths)
                .unwrap_or_else(|| external_drop_paths(&data));
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "drop_on_remote",
                json!([side, -1, paths, ctrl, shift]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_drop_tab_on_pane(move |pane_id, data| {
            let Some(tab_id) = tab_drag_payload(&data) else {
                return;
            };
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "move_tab_to_pane",
                json!([tab_id, pane_id]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_drop_tab_split(move |data, pane_id, direction| {
            let Some(tab_id) = tab_drag_payload(&data) else {
                return;
            };
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "place_tab_split",
                json!([tab_id, pane_id, direction.as_str()]),
            );
        });
    }
}

/// Group 4: window chrome, tab lifecycle, quick connect, logging, panels and
/// the small settings toggles that live on MainWindow (dialog-owned controls
/// are wired by `remote_dialog_wire` and are intentionally skipped here).
fn wire_shell_window_callbacks(
    window: &MainWindow,
    window_id: u64,
    client: &Arc<HostShellClient>,
    rt: &AppRuntimeClient,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
    shell: &Weak<RefCell<ShellWindows>>,
) {
    // Windows title-bar native drag.
    {
        let weak = window.as_weak();
        window.on_title_drag_requested(move || {
            if let Some(window) = weak.upgrade() {
                crate::win_chrome::start_native_window_move(window.window());
                crate::win_chrome::reset_pointer_state_after_native_move(window.as_weak());
            }
        });
    }
    // Full screen (Slint has no fullscreen property; use the Window API).
    {
        let weak = window.as_weak();
        window.on_toggle_fullscreen(move || {
            if let Some(window) = weak.upgrade() {
                let next = !window.window().is_fullscreen();
                window.window().set_fullscreen(next);
                window.set_window_fullscreen(next);
            }
        });
    }
    wire_call2!(window_id, window, rt, client, on_rename_tab, "rename_tab");
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_tab_context_menu_requested(move |tab_id, _x, _y| {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "prepare_tab_context_menu",
                json!([tab_id.as_str()]),
            );
        });
    }
    // Multi-window (N7): "Move Tab to Main Window" / "... to New Window".
    {
        let client = Arc::clone(client);
        let shell = shell.clone();
        window.on_move_tab_to_main_window(move |tab_id| {
            if let Some(shell) = shell.upgrade() {
                move_tab_to_main_window(&shell, &client, window_id, tab_id.as_str());
            }
        });
    }
    {
        let client = Arc::clone(client);
        let shell = shell.clone();
        window.on_move_tab_to_new_window(move |tab_id| {
            if let Some(shell) = shell.upgrade() {
                move_tab_to_new_window(&shell, &client, window_id, tab_id.as_str());
            }
        });
    }
    // Quick connect input helpers.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_quick_connect_clear_input(move || {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "update_quick_connect_input",
                json!([""]),
            );
        });
    }
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_quick_connect_copy(move |target| {
            write_clipboard(&clipboard, target.as_str());
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "quick_connect_copied_status",
                json!([target.as_str()]),
            );
        });
    }
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_open_saved_session,
        "open_first_saved_session"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_clear_saved_session_selection,
        "clear_saved_selection"
    );
    // Logging (the logging *dialog* is a separate OS window).
    {
        let rt = rt.clone();
        window.on_terminal_logging_open_file(move || {
            if let Some(path) = call_quiet(&rt, "active_session_logging_path", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                crate::bootstrap::spawn_open_path(path);
            }
        });
    }
    {
        let rt = rt.clone();
        window.on_terminal_logging_open_folder(move || {
            if let Some(path) = call_quiet(&rt, "active_session_logging_path", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                if path.is_empty() {
                    return;
                }
                let folder = std::path::Path::new(&path)
                    .parent()
                    .map(|parent| parent.display().to_string())
                    .unwrap_or(path);
                crate::bootstrap::spawn_open_path(folder);
            }
        });
    }
    wire_call1!(
        window_id,
        window,
        rt,
        client,
        on_set_debug_logging,
        "set_debug_logging"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_save_global_logging_settings,
        "save_global_logging_settings"
    );
    {
        let rt = rt.clone();
        window.on_open_debug_log_dir(move || {
            if let Some(dir) = call_quiet(&rt, "ensure_debug_log_dir", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
            {
                crate::bootstrap::spawn_open_path(dir);
            }
        });
    }
    {
        let rt = rt.clone();
        let weak = window.as_weak();
        let clipboard = Rc::clone(clipboard);
        window.on_copy_connection_overlay_error(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let text = call_quiet(&rt, "connection_overlay_error_text", json!([]))
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            if text.is_empty() {
                return;
            }
            write_clipboard(&clipboard, &text);
            window.set_status_text("Copied the connection error.".into());
        });
    }
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_manage_editor_proxies,
        "open_proxies_manager"
    );
    // Dock panel layout.
    wire_call2!(
        window_id,
        window,
        rt,
        client,
        on_panel_action,
        "panel_action"
    );
    wire_call2!(
        window_id,
        window,
        rt,
        client,
        on_panel_area_resized,
        "panel_area_resized"
    );
    wire_call2!(
        window_id,
        window,
        rt,
        client,
        on_panel_width_preview,
        "panel_width_preview"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_panel_width_commit,
        "panel_width_commit"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_panel_split_preview,
        "panel_split_preview"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_panel_split_commit,
        "panel_split_commit"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_panel_drag_start,
        "panel_drag_start"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_panel_drag_move,
        "panel_drag_move"
    );
    wire_call3!(
        window_id,
        window,
        rt,
        client,
        on_panel_drag_drop,
        "panel_drag_drop"
    );
    wire_call0!(
        window_id,
        window,
        rt,
        client,
        on_panel_drag_cancel,
        "panel_drag_cancel"
    );
    // Language: apply the bundled translation after the Host accepts it.
    {
        let rt = rt.clone();
        let client = Arc::clone(client);
        let weak = window.as_weak();
        window.on_set_ui_language(move |value| {
            call_and_refresh(
                window_id,
                &rt,
                &client,
                &weak,
                "set_ui_language",
                json!([value.as_str()]),
            );
            if let Some(window) = weak.upgrade() {
                crate::bootstrap::apply_language(window.get_settings_language_text().as_str());
            }
        });
    }
}

fn arg_json<T: ArgToJson>(value: &T) -> serde_json::Value {
    value.to_json()
}

trait ArgToJson {
    fn to_json(&self) -> serde_json::Value;
}

impl ArgToJson for slint::SharedString {
    fn to_json(&self) -> serde_json::Value {
        json!(self.as_str())
    }
}

impl ArgToJson for i32 {
    fn to_json(&self) -> serde_json::Value {
        json!(*self)
    }
}

impl ArgToJson for f32 {
    fn to_json(&self) -> serde_json::Value {
        json!(*self)
    }
}

impl ArgToJson for bool {
    fn to_json(&self) -> serde_json::Value {
        json!(*self)
    }
}

fn render_wire_snapshot(
    renderer: &Rc<RefCell<TerminalRenderer>>,
    snap: &WireTerminalSnapshot,
) -> Option<slint::Image> {
    let cols = snap.columns as usize;
    if cols == 0 || snap.rows == 0 {
        return None;
    }
    let mut lines = Vec::with_capacity(snap.rows as usize);
    for row in 0..snap.rows as usize {
        let start = row * cols;
        let end = (start + cols).min(snap.cells.len());
        if start >= snap.cells.len() {
            break;
        }
        let mut cells = Vec::with_capacity(cols);
        for wire in &snap.cells[start..end] {
            cells.push(TerminalCell {
                grapheme: wire.grapheme.clone(),
                foreground: TerminalColor::rgb(wire.fg[0], wire.fg[1], wire.fg[2]),
                background: TerminalColor::rgb(wire.bg[0], wire.bg[1], wire.bg[2]),
                bold: wire.bold,
                italic: wire.italic,
                underline: wire.underline,
                wide_continuation: wire.wide_continuation,
            });
        }
        while cells.len() < cols {
            cells.push(TerminalCell::default());
        }
        lines.push(std::borrow::Cow::Owned(cells));
    }
    let snapshot = TerminalSnapshot {
        lines,
        cursor: snap.cursor,
        // Item A: the Host selection is already in viewport coordinates, which
        // is exactly what the renderer paints (selection highlight only — copy /
        // select-all / drag stay Host-side).
        selection: snap.selection.map(WireSelection::to_selection_range),
    };
    let frame = renderer.borrow_mut().render(&snapshot);
    Some(slint::Image::from_rgba8(
        SharedPixelBuffer::clone_from_slice(&frame.rgba, frame.width, frame.height),
    ))
}
