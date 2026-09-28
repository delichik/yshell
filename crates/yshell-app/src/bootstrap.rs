//! Startup and app-shell bootstrap logic.

use std::{
    cell::Cell,
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use copypasta::{ClipboardContext, ClipboardProvider};
use slint::language::ColorScheme;
// Slint 1.18 只在私有 re-export 里暴露 `DragAction`（官方测试同样如此引用）；
// `data-transfer` 走公开的 `slint::DataTransfer`。
use slint::private_unstable_api::re_exports::DragAction;
use slint::{
    ComponentHandle, Model, ModelRc, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel,
};
use yshell_config::{PanelId, PanelSide};
use yshell_ssh::TransportBackend;
use yshell_terminal::{TerminalRenderer, DEFAULT_FONT_SIZE};
use yshell_ui::appearance::{self, AccentColors, ThemeMode};

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::runtime::{
    AppProjection, AppRuntime, AuthKeyOptionData, AuthPromptQuestionData, HostKeyGroupData,
    PanelFrameData as RuntimePanelFrameData, PrivateKeyRowData, SftpCrumbData, SftpRowData,
    SplitHandleData as RuntimeSplitHandleData, TabData, TransferRowData,
};

mod generated_ui {
    #![allow(dead_code)]
    slint::include_modules!();
}

use generated_ui::{
    AuthKeyOption, AuthKeyboardPrompt, HostKeyEntry, HostKeyGroup, LocalRow, MainWindow, Palette,
    PanelFrameData, PrivateKeyRow, QuickConnectRow, QuickLinkRow, SessionTreeRow, SftpCrumb,
    SftpRow, SftpTransferRow, SplitHandleData, TerminalScrollInfo, Theme, WorkspaceTab,
};

/// Keeps the pixel-rendered terminal surface in sync with the runtime.
///
/// This is cloned into every UI callback; `refresh` is cheap when the frame
/// revision did not change.
///
/// The renderer rasterizes at **physical** pixels while Slint lays the UI out
/// in **logical** pixels; the window scale factor bridges the two. It is kept
/// here so the grid math and the pointer mapping use the same value as the
/// renderer (`terminal_view.slint` sizes the bitmap with `phx`, i.e. it also
/// resolves the scale itself).
#[derive(Clone)]
struct TerminalSurface {
    runtime: Rc<RefCell<AppRuntime>>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    last_frame: Rc<RefCell<Option<(String, u64)>>>,
    scale_factor: Rc<Cell<f32>>,
}

impl TerminalSurface {
    fn new(runtime: Rc<RefCell<AppRuntime>>, renderer: Rc<RefCell<TerminalRenderer>>) -> Self {
        let scale_factor = renderer.borrow().scale_factor();
        Self {
            runtime,
            renderer,
            last_frame: Rc::new(RefCell::new(None)),
            scale_factor: Rc::new(Cell::new(sanitize_scale_factor(scale_factor))),
        }
    }

    /// Physical-pixel size of one cell (the renderer measures at `scale`).
    fn cell_size(&self) -> (u32, u32) {
        self.renderer.borrow().cell_size()
    }

    fn scale_factor(&self) -> f32 {
        self.scale_factor.get()
    }

    /// Adopt the window's device pixel ratio.
    ///
    /// Returns `true` when it changed: the renderer re-measures its physical
    /// cell metrics and drops its glyph cache, and the next `refresh` re-renders
    /// the frame (the cached frame revision is invalidated). The PTY grid is
    /// re-synced by the terminal poll timer, which runs every 120 ms.
    fn sync_scale_factor(&self, window: &MainWindow) -> bool {
        let scale_factor = sanitize_scale_factor(window.window().scale_factor());
        if scale_factor == self.scale_factor.get() {
            return false;
        }
        self.renderer.borrow_mut().set_scale_factor(scale_factor);
        self.scale_factor.set(scale_factor);
        *self.last_frame.borrow_mut() = None;
        true
    }

    /// Re-render the terminal image when the active session's frame changed.
    fn refresh(&self, window: &MainWindow) {
        let session = window.get_active_session().to_string();
        let frame_id = {
            let Ok(runtime) = self.runtime.try_borrow() else {
                return;
            };
            runtime.active_terminal_frame_id()
        };
        if let Some((last_session, last_frame)) = self.last_frame.borrow().as_ref() {
            if *last_session == session && *last_frame == frame_id {
                return;
            }
        }
        let Ok(runtime) = self.runtime.try_borrow() else {
            return;
        };
        let Some(snapshot) = runtime.active_terminal_render_snapshot() else {
            return;
        };
        let frame = self.renderer.borrow_mut().render(&snapshot);
        drop(snapshot);
        drop(runtime);
        let image = slint::Image::from_rgba8(SharedPixelBuffer::clone_from_slice(
            &frame.rgba,
            frame.width,
            frame.height,
        ));
        window.set_terminal_image(image);
        *self.last_frame.borrow_mut() = Some((session, frame_id));
    }
}

#[derive(Debug)]
pub struct YShellApp {
    pub state: AppState,
}

impl YShellApp {
    pub fn run(self) -> AppResult<()> {
        let window = MainWindow::new().map_err(AppError::from_error)?;
        apply_appearance(&window);
        apply_fonts(&window);
        apply_language();
        // W5-A2：窗口级快捷键依赖 root FocusScope 持有键盘焦点（终端/弹窗会接管焦点，
        // 终端聚焦时的组合键由 `dispatch_global_shortcut` 兜底）。
        window.invoke_focus_global_shortcuts();
        let runtime = Rc::new(RefCell::new(self.state.runtime));
        // `window.window().scale_factor()` is the best value available before the
        // window is mapped; `sync_scale_factor` after `show()` picks up the final
        // winit scale (Xft.dpi / WINIT_X11_SCALE_FACTOR) and re-renders.
        let renderer = Rc::new(RefCell::new(TerminalRenderer::with_scale_factor(
            DEFAULT_FONT_SIZE,
            window.window().scale_factor(),
        )));
        let surface = TerminalSurface::new(Rc::clone(&runtime), renderer);
        // N2：启动落点 = Quick Connect 页（无标签时内容区显示 QC 页，不新建标签，
        // 见 `AppRuntime::quick_connect_visible`）。
        let initial_projection = resolve_startup_ssh_backend(&mut runtime.borrow_mut());
        apply_projection(&window, &initial_projection);
        surface.refresh(&window);
        // N1：SFTP 传输 worker（worker 线程 + mpsc，120ms UI 定时器 drain）。
        let (sftp_job_tx, sftp_job_rx) = crate::sftp_jobs::start_sftp_job_worker();
        runtime.borrow_mut().sftp_jobs = Some(crate::sftp_jobs::SftpJobHandle::new(sftp_job_tx));
        // 本地栏首次列出（默认 home 目录）。
        let local_projection = runtime.borrow_mut().refresh_local_pane();
        apply_projection(&window, &local_projection);
        let dialog_rx = wire_callbacks(&window, Rc::clone(&runtime), surface.clone());
        let _terminal_poll_timer =
            start_terminal_poll_timer(&window, runtime, surface.clone(), dialog_rx, sftp_job_rx);
        window.show().map_err(AppError::from_error)?;
        if surface.sync_scale_factor(&window) {
            surface.refresh(&window);
        }
        window.run().map_err(AppError::from_error)
    }
}

/// E2E/开发开关：`YSHELL_SSH_BACKEND=native-ssh|fake` 启动即切换传输后端，
/// 等价于调用 `select_native_ssh_transport_backend()` / `select_fake_transport_backend()`。
///
/// S2/D26：**未设置或值无法识别时默认原生 SSH（Real）**，桌面启动不再落到 fake
/// 适配器；`fake` 仅供 e2e/离线测试显式选择。
fn resolve_startup_ssh_backend(runtime: &mut AppRuntime) -> AppProjection {
    match startup_ssh_backend_from_value(std::env::var("YSHELL_SSH_BACKEND").ok().as_deref()) {
        // 显式开关（e2e/开发）：沿用菜单同款切换路径与状态文案。
        Some(TransportBackend::Fake) => runtime.select_fake_transport_backend(),
        Some(TransportBackend::Real) => runtime.select_native_ssh_transport_backend(),
        // 无环境变量：桌面启动默认原生 SSH，状态文案保持 `startup_status()`。
        None => runtime.prepare_desktop_startup_projection(),
    }
}

/// `YSHELL_SSH_BACKEND` 的取值解析（大小写与首尾空白不敏感；未知值回退 `None`）。
fn startup_ssh_backend_from_value(value: Option<&str>) -> Option<TransportBackend> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "native-ssh" => Some(TransportBackend::Real),
        "fake" => Some(TransportBackend::Fake),
        _ => None,
    }
}

/// 解析外观设置（W1 用环境变量作为验收开关；`config.toml [appearance]` 接线在 W4/W5）。
///
/// `YSHELL_THEME`：`system`（默认）/ `light` / `dark`
/// `YSHELL_ACCENT`：强调色预设 id（`blue` 默认，另有 teal/purple/green/orange/magenta）
fn resolve_appearance(
    theme_env: Option<&str>,
    accent_env: Option<&str>,
) -> (ThemeMode, AccentColors) {
    let mode = theme_env.and_then(ThemeMode::from_id).unwrap_or_default();
    let accent = appearance::resolve_accent(accent_env.unwrap_or_default());
    (mode, accent)
}

/// 把解析后的主题模式与强调色写入 Slint 的 `Theme` 全局。
///
/// 明暗的"跟随系统"由 Slint 侧处理（`Theme.mode = unknown` 时读取
/// std-widgets 的 `Palette.color-scheme`），Rust 只负责用户显式选择的模式。
fn apply_appearance(window: &MainWindow) {
    let (mode, accent) = resolve_appearance(
        std::env::var("YSHELL_THEME").ok().as_deref(),
        std::env::var("YSHELL_ACCENT").ok().as_deref(),
    );

    let theme = window.global::<Theme>();
    theme.set_mode(match mode {
        ThemeMode::System => 0,
        ThemeMode::Light => 1,
        ThemeMode::Dark => 2,
    });

    // 显式浅色/深色时同步覆盖 std-widgets 的色板，保证整窗一致；跟随系统时不动。
    // T0：Slint 1.18.1 起 `slint::language::ColorScheme` 是公开类型（1.9 只能用
    // `private_unstable_api` 的 re-export）；W3 用自绘控件替换 std-widgets 后即可删除这段。
    match mode {
        ThemeMode::System => {}
        ThemeMode::Light => window
            .global::<Palette>()
            .set_color_scheme(ColorScheme::Light),
        ThemeMode::Dark => window
            .global::<Palette>()
            .set_color_scheme(ColorScheme::Dark),
    }

    let to_color =
        |value: appearance::RgbColor| slint::Color::from_rgb_u8(value.red, value.green, value.blue);
    theme.set_accent_light(to_color(accent.light));
    theme.set_accent_dark(to_color(accent.dark));
    theme.set_accent_hover_light(to_color(accent.hover_light));
    theme.set_accent_hover_dark(to_color(accent.hover_dark));
    theme.set_accent_pressed_light(to_color(accent.pressed_light));
    theme.set_accent_pressed_dark(to_color(accent.pressed_dark));
    theme.set_accent_text_light(to_color(accent.text_light));
    theme.set_accent_text_dark(to_color(accent.text_dark));
    theme.set_accent_soft_light(to_color(accent.soft_light));
    theme.set_accent_soft_dark(to_color(accent.soft_dark));
}

/// 平台默认 UI 字体族（必须同时覆盖拉丁与 CJK）。
///
/// Slint 1.9 的 `font-family` 只接受单一字体族（不支持逗号列表），软件渲染器也没有
/// 逐字形回退，所以不能像 Web 那样写字体链（见设计规范 §7.5）。
fn platform_default_ui_font() -> &'static str {
    if cfg!(target_os = "windows") {
        "Microsoft YaHei UI"
    } else if cfg!(target_os = "macos") {
        "PingFang SC"
    } else {
        "Noto Sans SC"
    }
}

/// 解析 UI 字体族：`YSHELL_UI_FONT` 非空时优先（验收/调试用），否则按平台取默认。
///
/// 覆盖值来自运行时环境，无法保证 `'static`，故返回 `String`；空串或纯空白视为未覆盖。
fn resolve_ui_font(env_override: Option<&str>) -> String {
    match env_override.map(str::trim) {
        Some(font) if !font.is_empty() => font.to_owned(),
        _ => platform_default_ui_font().to_owned(),
    }
}

/// 把 UI 字体族写入 `Theme` 全局（须在窗口创建后、`run()` 前调用）。
fn apply_fonts(window: &MainWindow) {
    let font = resolve_ui_font(std::env::var("YSHELL_UI_FONT").ok().as_deref());
    window.global::<Theme>().set_font_ui(font.into());
}

/// 解析语言：显式 `zh-CN`/`en-US` 直通（大小写与首尾空白不敏感）；
/// 其它（含 `system`）按系统 locale 判断，均未知时回退 `en-US`。
fn resolve_language(requested: &str, system_locale: Option<&str>) -> &'static str {
    match requested.trim().to_ascii_lowercase().as_str() {
        "zh-cn" | "zh" | "zh-hans" => "zh-CN",
        "en-us" | "en" => "en-US",
        _ => {
            let locale = system_locale.unwrap_or_default().to_ascii_lowercase();
            if locale.starts_with("zh") {
                "zh-CN"
            } else {
                "en-US"
            }
        }
    }
}

/// 应用界面语言（W4：`YSHELL_LANG=system|zh-CN|en-US` 作为验收开关；
/// 设置页下拉在 W5 接线到同一函数）。
fn apply_language() {
    let requested = std::env::var("YSHELL_LANG").unwrap_or_default();
    let system_locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LANG"))
        .ok();
    let language = resolve_language(&requested, system_locale.as_deref());
    if let Err(error) = slint::select_bundled_translation(language) {
        eprintln!("failed to select bundled translation '{language}': {error}");
    }
}

fn wire_callbacks(
    window: &MainWindow,
    runtime: Rc<RefCell<AppRuntime>>,
    surface: TerminalSurface,
) -> std::sync::mpsc::Receiver<FileDialogOutcome> {
    // Never shadow this binding: each callback clones a fresh handle.
    let surface_source = surface;
    let clipboard = Rc::new(RefCell::new(ClipboardContext::new().ok()));
    // N4：rfd 文件对话框结果（worker 线程 → mpsc → 终端轮询定时器 drain）。
    let (dialog_tx, dialog_rx) = std::sync::mpsc::channel::<FileDialogOutcome>();

    // N4：投影型回调的样板（每次回调克隆 weak/runtime/surface 并应用投影）。
    macro_rules! wire_n4 {
        ($setter:ident, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {
            {
                let weak = window.as_weak();
                let runtime_ref = Rc::clone(&runtime);
                let surface = surface_source.clone();
                window.$setter(move |$($arg : $ty),*| {
                    if let Some(window) = weak.upgrade() {
                        let $runtime = &mut runtime_ref.borrow_mut();
                        let projection = $body;
                        apply_projection(&window, &projection);
                        surface.refresh(&window);
                    }
                });
            }
        };
    }
    // N4：`AppResult<AppProjection>` 型回调（错误 → 状态栏）。
    macro_rules! wire_n4_result {
        ($setter:ident, $error_prefix:expr, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {
            {
                let weak = window.as_weak();
                let runtime_ref = Rc::clone(&runtime);
                let surface = surface_source.clone();
                window.$setter(move |$($arg : $ty),*| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    let result = {
                        let $runtime = &mut runtime_ref.borrow_mut();
                        $body
                    };
                    match result {
                        Ok(projection) => {
                            apply_projection(&window, &projection);
                            surface.refresh(&window);
                        }
                        Err(error) => {
                            set_plain_status(&window, format!("{}: {error}", $error_prefix).into());
                        }
                    }
                });
            }
        };
    }

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_quick_connect(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().open_quick_connect_tab();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_quick_connect_input(move |text| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_quick_connect_input(text.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_submit_quick_connect(move |input| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .submit_quick_connect(input.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
                // 解析失败（内联错误态）时把焦点留在输入框，方便直接改。
                if !window.get_quick_connect_error_text().is_empty() {
                    window.invoke_focus_quick_connect();
                }
            }
            Err(error) => {
                set_plain_status(&window, format!("Quick Connect error: {error}").into());
                window.set_status_kind_text("quick-connect-error".into());
                window.set_status_param_1_text(error.to_string().into());
                window.set_status_param_2_text("".into());
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_quick_connect_pin(move |target| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().quick_connect_pin(target.as_ref()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Pin quick link error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_quick_link_remove(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().quick_link_remove(id.as_ref()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Remove quick link error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let clipboard_ref = Rc::clone(&clipboard);
    window.on_quick_connect_copy(move |target| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if let Some(clipboard) = clipboard_ref.borrow_mut().as_mut() {
            let _ = clipboard.set_contents(target.to_string());
        }
        let projection = runtime_ref
            .borrow_mut()
            .quick_connect_copied_status(target.as_ref());
        apply_projection(&window, &projection);
        surface.refresh(&window);
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_quick_connect_save_as_session(move |target| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref
            .borrow_mut()
            .start_quick_connect_session_editor(target.as_ref())
        {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Save as session error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_quick_connect_clear_history(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().quick_connect_history_clear() {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Clear history error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_new_tab_default(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().handle_new_tab_default();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_activate_tab(move |tab_id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().activate_tab(tab_id.as_ref()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Activate tab error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_close_tab(move |tab_id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().request_close_tab(tab_id.as_ref()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Close tab error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_close_tabs(move |scope| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().request_close_tabs(scope.as_ref()) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Close tabs error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_tab_context_menu_requested(move |tab_id, _x, _y| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .prepare_tab_context_menu(tab_id.as_ref());
            apply_projection(&window, &projection);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_confirm_close_tabs(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match runtime_ref.borrow_mut().confirm_close_tabs() {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Confirm close tabs error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_cancel_close_tabs(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().cancel_close_tabs();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_active_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Save session error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().open_first_saved_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Open saved session error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_session_search(move |query| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_session_search(query.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_selected_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().open_selected_saved_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(
                &window,
                format!("Open selected saved session error: {error}").into(),
            ),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_selected_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .update_selected_saved_session_from_active();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(
                &window,
                format!("Update selected saved session error: {error}").into(),
            ),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_delete_selected_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().delete_selected_saved_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(
                &window,
                format!("Delete selected saved session error: {error}").into(),
            ),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_previous_saved_session(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_previous_saved_session();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_next_saved_session(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_next_saved_session();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    // 会话树输入状态：重复点击兜底 + 激活去重（见 SessionTreeInput 注释）。
    let tree_input: Rc<RefCell<SessionTreeInput>> =
        Rc::new(RefCell::new(SessionTreeInput::default()));

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let tree_input_ref = Rc::clone(&tree_input);
    window.on_select_saved_session(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // 单击 = 选中；在重复点击窗口内再点同一节点 = 双击，升级为激活。
        let repeated = session_tree_register_click(&tree_input_ref, id.as_ref());
        let result = if repeated {
            session_tree_activate(&tree_input_ref, &runtime_ref, id.as_ref())
        } else {
            Some(Ok(runtime_ref
                .borrow_mut()
                .select_saved_session_by_id(id.as_ref())))
        };
        let Some(result) = result else {
            return;
        };
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Open saved session error: {error}").into())
            }
        }
    });

    // 右键菜单前的选中：只改选中，不登记点击（不参与双击兜底）。
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_saved_session_for_menu(move |id| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .select_saved_session_by_id(id.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_saved_folder(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().toggle_saved_folder(id.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Toggle folder error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let tree_input_ref = Rc::clone(&tree_input);
    window.on_activate_saved_session_tree_node(move |id| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // 原生 double-clicked / 回车：与重复点击兜底共用去重窗口。
        let Some(result) = session_tree_activate(&tree_input_ref, &runtime_ref, id.as_ref()) else {
            return;
        };
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Open saved session error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_send_demo_input(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().send_active_terminal_input("pwd\n");
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal input error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_send_terminal_input(move |input| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let text = input.to_string();
        if text.is_empty() {
            return;
        }
        let payload = if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        };
        let result = runtime_ref
            .borrow_mut()
            .send_active_terminal_input(&payload);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal input error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let clipboard_ref = Rc::clone(&clipboard);
    window.on_copy_terminal_visible(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().copy_active_terminal_visible_text();
        match result {
            Ok(projection) => {
                if let Some(clipboard) = clipboard_ref.borrow_mut().as_mut() {
                    let _ = clipboard
                        .set_contents(runtime_ref.borrow().terminal_clipboard_text().to_owned());
                }
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Copy terminal error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let clipboard_ref = Rc::clone(&clipboard);
    window.on_copy_terminal_selection(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match copy_selection_to_clipboard(&runtime_ref, &clipboard_ref) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Copy selection error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_select_all(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().select_all_active_terminal();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Select all error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let clipboard_ref = Rc::clone(&clipboard);
    window.on_paste_terminal_clipboard(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        match paste_clipboard_into_terminal(&runtime_ref, &clipboard_ref) {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Paste terminal error: {error}").into())
            }
        }
    });

    // --- Terminal keyboard / scroll / selection surface ---------------------
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let clipboard_ref = Rc::clone(&clipboard);
    window.on_terminal_key(move |text, ctrl, alt, shift, meta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // W5-A2：终端 FocusScope 会消费全部按键（见 ui/components/terminal_view.slint），
        // 所以终端聚焦时的窗口级快捷键在这里兜底转发给同一份分发函数。
        if dispatch_global_shortcut(&window, text.as_ref(), ctrl, shift, alt, meta) {
            return;
        }
        let result = handle_terminal_key(
            &runtime_ref,
            &clipboard_ref,
            TerminalKeyPress {
                text: text.as_ref(),
                ctrl,
                alt,
                shift,
                meta,
            },
        );
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Terminal key error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_scroll(move |delta| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().scroll_active_terminal(delta);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal scroll error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_scroll_to_bottom(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().scroll_active_terminal_to_bottom();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal scroll error: {error}").into())
            }
        }
    });

    // 滚动条（§5.17）拖拽/点击轨道：换算成"距顶部行号"后定位视口。
    // 通道：`TerminalScrollInfo` 是 ui/components/terminal_view.slint 里声明的
    // 全局单例，终端视图直接读它的行数、直接调它的回调，因此不依赖
    // ui/main_window.slint 再转发一层。
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window
        .global::<TerminalScrollInfo>()
        .on_scroll_to_line(move |line| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let result = runtime_ref
                .borrow_mut()
                .scroll_active_terminal_to_line(line);
            match result {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("Terminal scroll error: {error}").into())
                }
            }
        });

    let suppress_selection_end = Rc::new(Cell::new(false));

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_selection_begin(move |x, y| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some((column, row)) = terminal_grid_point(&surface, &runtime_ref, x, y) else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .begin_active_terminal_selection(column, row);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal selection error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_selection_update(move |x, y| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some((column, row)) = terminal_grid_point(&surface, &runtime_ref, x, y) else {
            return;
        };
        let _ = runtime_ref
            .borrow_mut()
            .update_active_terminal_selection(column, row);
        let projection = runtime_ref.borrow().projection();
        apply_projection(&window, &projection);
        surface.refresh(&window);
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let suppress_ref = Rc::clone(&suppress_selection_end);
    window.on_terminal_selection_end(move |x, y| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        if suppress_ref.replace(false) {
            return;
        }
        let Some((column, row)) = terminal_grid_point(&surface, &runtime_ref, x, y) else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .update_active_terminal_selection(column, row);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal selection error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    let suppress_ref = Rc::clone(&suppress_selection_end);
    window.on_terminal_select_word_at(move |x, y| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let Some((column, row)) = terminal_grid_point(&surface, &runtime_ref, x, y) else {
            return;
        };
        // The release event that follows a double click must not override the
        // word selection with an empty drag range.
        suppress_ref.set(true);
        let result = runtime_ref
            .borrow_mut()
            .select_word_in_active_terminal(column, row);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Terminal selection error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_clear_terminal(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().clear_active_terminal();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Clear terminal error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_find_terminal_text(move |query| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .find_in_active_terminal(query.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Find terminal error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_find_terminal_next(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().select_next_terminal_match();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Find next error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_find_terminal_prev(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().select_previous_terminal_match();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Find previous error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_refresh_sftp(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().refresh_active_sftp_listing();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP refresh error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_sftp_path(move |path| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().open_sftp_path(path.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Open SFTP path error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_sftp_parent(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().open_sftp_parent();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP up error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_upload_sftp(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.upload_sftp_file() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP upload error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_download_sftp(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.download_sftp_file() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP download error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_create_sftp_directory(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.create_sftp_directory() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP mkdir error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_rename_sftp_path(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.rename_sftp_path() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP rename error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_chmod_sftp_path(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.chmod_sftp_path() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP chmod error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_start_sftp_remote_edit(
        move |local_path, remote_target, secondary_target, permissions| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut runtime = runtime_ref.borrow_mut();
            let local_path = local_path.to_string();
            let remote_target = remote_target.to_string();
            let secondary_target = secondary_target.to_string();
            let permissions = permissions.to_string();
            runtime.set_sftp_operation_inputs(
                &local_path,
                &remote_target,
                &secondary_target,
                &permissions,
            );
            match runtime.start_sftp_remote_edit() {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP remote edit error: {error}").into())
                }
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_sftp_remote_edit(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_sftp_remote_edit();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(
                &window,
                format!("SFTP remote edit save error: {error}").into(),
            ),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_cancel_sftp_remote_edit(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().cancel_sftp_remote_edit();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(
                &window,
                format!("SFTP remote edit cancel error: {error}").into(),
            ),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_select_sftp_entry(move |index| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let projection = runtime_ref.borrow_mut().select_sftp_entry(index);
        apply_projection(&window, &projection);
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_activate_sftp_entry(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().activate_sftp_entry();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP open error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_sort_sftp(move |column| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let projection = runtime_ref.borrow_mut().sort_sftp_by(column.as_ref());
        apply_projection(&window, &projection);
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    window.on_toggle_sftp_hidden(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let projection = runtime_ref.borrow_mut().toggle_sftp_hidden_files();
        apply_projection(&window, &projection);
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_sftp_crumb(move |index| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().open_sftp_crumb(index);
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("SFTP breadcrumb error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_create_sftp_folder(move |name| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .create_sftp_folder_named(name.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP mkdir error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_rename_sftp_selected(move |new_name| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .rename_sftp_selected(new_name.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP rename error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_delete_sftp_selected(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().delete_sftp_selected();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP delete error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_chmod_sftp_selected(move |permissions| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .chmod_sftp_selected(permissions.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP chmod error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_upload_sftp_into_current(move |local_path| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .upload_sftp_into_current(local_path.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP upload error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_download_sftp_selected(move |local_path| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .download_sftp_selected(local_path.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("SFTP download error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_edit_sftp_selected(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().edit_sftp_selected();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("SFTP remote edit error: {error}").into())
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_disconnect_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().disconnect_active_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Disconnect error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_reconnect_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().reconnect_active_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Reconnect error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_secret_reset_confirmation(move |confirmation| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_secret_reset_confirmation(confirmation.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_reset_secrets(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().reset_secret_store();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Reset secrets error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_open_known_hosts_manager(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().open_known_hosts_manager();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_close_known_hosts_manager(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().close_known_hosts_manager();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_previous_known_host(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_previous_known_host();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_next_known_host(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_next_known_host();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_remove_selected_known_host(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().remove_selected_known_host();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Known hosts error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_known_hosts_clear_confirmation(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_known_hosts_clear_confirmation(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_clear_all_known_hosts(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().clear_all_known_hosts();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Known hosts error: {error}").into()),
        }
    });

    // --- N4：私钥管理页 -----------------------------------------------------
    wire_n4!(on_open_private_keys_manager, |runtime| runtime
        .open_private_keys_manager());
    wire_n4!(on_close_private_keys_manager, |runtime| runtime
        .close_private_keys_manager());
    wire_n4!(on_select_private_key, |runtime, index: i32| runtime
        .select_private_key(index));
    wire_n4!(
        on_update_private_key_import_label,
        |runtime, value: SharedString| runtime.update_private_keys_import_label(value.as_ref())
    );
    wire_n4!(
        on_update_private_key_import_path,
        |runtime, value: SharedString| runtime.update_private_keys_import_path(value.as_ref())
    );
    wire_n4!(
        on_update_private_key_import_passphrase,
        |runtime, value: SharedString| runtime
            .update_private_keys_import_passphrase(value.as_ref())
    );
    wire_n4!(
        on_toggle_private_key_remember_import_passphrase,
        |runtime, checked: bool| runtime.toggle_private_keys_remember_import_passphrase(checked)
    );
    wire_n4!(on_import_private_key, |runtime| runtime
        .import_private_key_from_form());
    wire_n4!(on_test_private_key, |runtime| runtime
        .test_selected_private_key());
    wire_n4!(on_request_private_key_remove, |runtime| runtime
        .request_private_key_remove());
    wire_n4!(on_confirm_private_key_remove, |runtime| runtime
        .confirm_private_key_remove());
    wire_n4!(on_cancel_private_key_remove, |runtime| runtime
        .cancel_private_key_remove());
    wire_n4!(on_request_private_key_deploy, |runtime| runtime
        .request_private_key_deploy());
    wire_n4!(on_confirm_private_key_deploy, |runtime| runtime
        .confirm_private_key_deploy());
    wire_n4!(on_cancel_private_key_deploy, |runtime| runtime
        .cancel_private_key_deploy());
    wire_n4!(
        on_update_private_key_passphrase,
        |runtime, value: SharedString| runtime.update_private_keys_passphrase_input(value.as_ref())
    );
    wire_n4!(on_save_private_key_passphrase, |runtime| runtime
        .save_selected_private_key_passphrase());
    wire_n4!(on_forget_private_key_passphrase, |runtime| runtime
        .forget_selected_private_key_passphrase());
    wire_n4!(on_rename_private_key, |runtime, label: SharedString| {
        runtime.rename_selected_private_key(label.as_ref())
    });

    // N4：浏览私钥文件（rfd；worker 线程 + mpsc 回填）。
    {
        let dialog_tx = dialog_tx.clone();
        window.on_browse_private_key_file(move || {
            spawn_pick_file(
                dialog_tx.clone(),
                DialogKind::PrivateKeyImport,
                "Select an OpenSSH private key",
            );
        });
    }

    // N4：复制派生公钥到系统剪贴板（状态反馈走私钥页状态行）。
    {
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(&runtime);
        let clipboard_ref = Rc::clone(&clipboard);
        let surface = surface_source.clone();
        window.on_copy_private_key_public(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let result = runtime_ref.borrow().selected_private_key_public_line();
            let detail = match result {
                Ok(text) => {
                    let copied = clipboard_ref
                        .borrow_mut()
                        .as_mut()
                        .is_some_and(|clipboard| clipboard.set_contents(text.clone()).is_ok());
                    if copied {
                        format!(
                            "Copied the derived public key ({} chars) to the clipboard.",
                            text.len()
                        )
                    } else {
                        "Could not access the system clipboard; select the key text manually."
                            .to_owned()
                    }
                }
                Err(error) => format!("Copy failed: {error}"),
            };
            let projection = runtime_ref.borrow_mut().set_private_keys_status(&detail);
            apply_projection(&window, &projection);
            surface.refresh(&window);
        });
    }

    // N4：导出派生公钥（rfd 保存对话框，worker 线程写文件）。
    {
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(&runtime);
        let dialog_tx = dialog_tx.clone();
        window.on_export_private_key_public(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            match runtime_ref.borrow().selected_private_key_public_line() {
                Ok(text) => spawn_save_text(
                    dialog_tx.clone(),
                    DialogKind::PublicKeyExport,
                    "Export public key",
                    "yshell-public-key.pub",
                    text,
                ),
                Err(error) => {
                    let projection = runtime_ref
                        .borrow_mut()
                        .set_private_keys_status(&format!("Export failed: {error}"));
                    apply_projection(&window, &projection);
                }
            }
        });
    }

    // --- N4：主机密钥页 -----------------------------------------------------
    wire_n4!(on_open_host_keys_manager, |runtime| runtime
        .open_host_keys_manager());
    wire_n4!(on_close_host_keys_manager, |runtime| runtime
        .close_host_keys_manager());
    wire_n4!(on_select_host_key, |runtime, group: i32, entry: i32| {
        runtime.select_host_key(group, entry)
    });
    wire_n4_result!(on_remove_selected_host_key, "Host keys error", |runtime| {
        runtime.remove_selected_host_key()
    });
    wire_n4!(on_toggle_host_keys_import, |runtime| runtime
        .toggle_host_keys_import());
    wire_n4!(
        on_update_host_keys_import_text,
        |runtime, value: SharedString| runtime.update_host_keys_import_text(value.as_ref())
    );
    wire_n4!(on_import_host_keys, |runtime| runtime
        .import_host_keys_from_text());
    wire_n4!(
        on_update_host_keys_clear_confirmation,
        |runtime, value: SharedString| runtime.update_host_keys_clear_confirmation(value.as_ref())
    );
    wire_n4_result!(on_clear_all_host_keys, "Host keys error", |runtime| runtime
        .clear_all_host_keys());

    // N4：导出主机密钥（rfd 保存对话框；文本 = `host:port algorithm fingerprint`）。
    {
        let dialog_tx = dialog_tx.clone();
        let runtime_ref = Rc::clone(&runtime);
        window.on_export_host_keys(move || {
            let text = runtime_ref.borrow().export_host_keys_text();
            spawn_save_text(
                dialog_tx.clone(),
                DialogKind::HostKeysExport,
                "Export host keys",
                "yshell-known-hosts.txt",
                text,
            );
        });
    }

    // --- N4：认证弹窗 -------------------------------------------------------
    wire_n4!(on_select_auth_prompt_method, |runtime, index: i32| runtime
        .select_auth_prompt_method(index));
    wire_n4!(
        on_update_auth_prompt_password,
        |runtime, value: SharedString| runtime.update_auth_prompt_password(value.as_ref())
    );
    wire_n4!(
        on_toggle_auth_prompt_remember_password,
        |runtime, checked: bool| runtime.toggle_auth_prompt_remember_password(checked)
    );
    wire_n4!(on_select_auth_prompt_key, |runtime, index: i32| runtime
        .select_auth_prompt_key(index));
    wire_n4!(
        on_update_auth_prompt_passphrase,
        |runtime, value: SharedString| runtime.update_auth_prompt_passphrase(value.as_ref())
    );
    wire_n4!(on_toggle_auth_prompt_use_agent, |runtime, checked: bool| {
        runtime.toggle_auth_prompt_use_agent(checked)
    });
    wire_n4!(
        on_update_auth_prompt_keyboard_answer,
        |runtime, index: i32, value: SharedString| runtime
            .update_auth_prompt_keyboard_answer(index, value.as_ref())
    );
    wire_n4!(on_submit_auth_prompt, |runtime| runtime
        .submit_auth_prompt());
    wire_n4!(on_submit_auth_prompt_keyboard_round, |runtime| runtime
        .submit_auth_prompt_keyboard_round());
    wire_n4!(on_cancel_auth_prompt, |runtime| runtime
        .cancel_auth_prompt());

    // N4：浏览认证用的私钥文件（rfd）。
    {
        let dialog_tx = dialog_tx.clone();
        window.on_browse_auth_prompt_key_file(move || {
            spawn_pick_file(
                dialog_tx.clone(),
                DialogKind::AuthKeyFile,
                "Select a private key file",
            );
        });
    }

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_cancel_host_key_prompt(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().cancel_host_key_prompt();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_trust_host_key_once(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().trust_host_key_once();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Host key error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_trust_host_key_and_save(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().trust_host_key_and_save();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Host key error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_host_key_replace_confirmation(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_host_key_replace_confirmation(&value);
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_replace_host_key_and_connect(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().replace_host_key_and_connect();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Host key error: {error}").into()),
        }
    });

    // --- W5：连接密码弹窗回调 ------------------------------------------------
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_cancel_password_prompt(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().cancel_password_prompt();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_submit_password(move |password| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().submit_password(password.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                set_plain_status(&window, format!("Password prompt error: {error}").into());
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_start_new_saved_session_editor(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().start_new_saved_session_editor();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_close_session_editor_modal(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().close_session_editor_modal();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_new_folder_name(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_new_folder_name(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_create_folder_under_editor_target(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().create_folder_under_editor_target();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Create folder error: {error}").into()),
        }
    });

    // S1：会话树空白区菜单 —— 在根目录新建文件夹（名称来自弹窗，一次性传入）。
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_create_root_saved_folder(move |name| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .create_root_saved_folder(name.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Create folder error: {error}").into()),
        }
    });

    // S1：会话树空白区菜单 —— 重新读取 config.toml 并刷新会话树。
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_refresh_saved_sessions(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().refresh_saved_sessions();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Refresh error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_load_selected_saved_session_into_editor(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .load_selected_saved_session_into_editor();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Load editor error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_general(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_general();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_authentication(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .select_session_editor_authentication();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_terminal(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_terminal();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_sftp(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_sftp();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_tunnels(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_tunnels();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_proxy(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_proxy();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_logging(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_logging();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_advanced(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_advanced();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_session_editor_appearance(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_session_editor_appearance();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_previous_editor_folder(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_previous_editor_folder();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_next_editor_folder(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_next_editor_folder();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_name(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().update_editor_name(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_host(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().update_editor_host(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_port(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().update_editor_port(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_username(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_username(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_password(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_password(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_key_path(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_key_path(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_passphrase(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_passphrase(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_mode_none(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_proxy_mode_none();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_mode_custom(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_proxy_mode_custom();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_protocol_socks4(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_proxy_protocol_socks4();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_protocol_socks4a(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_proxy_protocol_socks4a();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_protocol_socks5(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_proxy_protocol_socks5();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_proxy_protocol_http_connect(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_editor_proxy_protocol_http_connect();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_proxy_host(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_proxy_host(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_proxy_port(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_proxy_port(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_proxy_username(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_proxy_username(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_proxy_password(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_proxy_password(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_editor_proxy_dns_by_proxy(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_editor_proxy_dns_by_proxy();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_tunnel_kind_local(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_tunnel_kind_local();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_tunnel_kind_remote(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_tunnel_kind_remote();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_tunnel_kind_dynamic(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_tunnel_kind_dynamic();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_tunnel_bind_host(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_tunnel_bind_host(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_tunnel_bind_port(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_tunnel_bind_port(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_tunnel_target_host(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_tunnel_target_host(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_editor_tunnel_target_port(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_editor_tunnel_target_port(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_add_editor_tunnel_forward(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().add_editor_tunnel_forward();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Add tunnel error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_clear_editor_tunnels(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().clear_editor_tunnels();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_global_logging_enabled(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_global_logging_enabled();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_global_logging_format_raw(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_global_logging_format_raw();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_global_logging_format_sanitized(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_global_logging_format_sanitized();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_global_logging_directory(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_global_logging_directory(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_global_logging_settings(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_global_logging_settings();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Save logging error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_settings_scrollback_lines(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_settings_scrollback_lines(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    // --- N6：终端日志（右键菜单/状态栏入口 + 弹窗）---------------------------
    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_logging_start(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().terminal_logging_start();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_terminal_logging_stop(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let projection = match runtime_ref.borrow_mut().terminal_logging_stop() {
            Ok(projection) => projection,
            Err(error) => {
                let mut runtime = runtime_ref.borrow_mut();
                runtime.set_status_kind(
                    "logging-error",
                    format!("Terminal logging failed: {error}"),
                    String::new(),
                    error.to_string(),
                );
                runtime.projection()
            }
        };
        apply_projection(&window, &projection);
        surface.refresh(&window);
    });

    // 打开日志文件/目录（系统默认程序；worker 线程避免阻塞 UI）。
    {
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(&runtime);
        window.on_terminal_logging_open_file(move || {
            if weak.upgrade().is_none() {
                return;
            }
            let path = runtime_ref.borrow().active_session_logging_path_text();
            spawn_open_path(path);
        });
    }
    {
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(&runtime);
        window.on_terminal_logging_open_folder(move || {
            if weak.upgrade().is_none() {
                return;
            }
            let path = runtime_ref.borrow().active_session_logging_path_text();
            if path.is_empty() {
                return;
            }
            let folder = std::path::Path::new(&path)
                .parent()
                .map(|parent| parent.display().to_string())
                .unwrap_or(path);
            spawn_open_path(folder);
        });
    }

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_cancel(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().close_logging_dialog();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_directory_changed(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_logging_dialog_directory(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_file_name_changed(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_logging_dialog_file_name(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_options_changed(
        move |raw, timestamps, include_input, input_confirmed, overwrite_confirmed| {
            if let Some(window) = weak.upgrade() {
                let projection = runtime_ref.borrow_mut().set_logging_dialog_options(
                    raw,
                    timestamps,
                    include_input,
                    input_confirmed,
                    overwrite_confirmed,
                );
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
        },
    );

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_start(move || {
        if let Some(window) = weak.upgrade() {
            if let Ok(projection) = runtime_ref.borrow_mut().start_logging_from_dialog() {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_logging_dialog_stop(move || {
        if let Some(window) = weak.upgrade() {
            if let Ok(projection) = runtime_ref.borrow_mut().stop_logging_from_dialog() {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
        }
    });

    // 弹窗"Browse…"：rfd 目录选择（worker 线程 + mpsc 回填；无显示服务器时降级为
    // 直接输入路径，rfd spike §10.3 的 display 预检）。
    {
        let dialog_tx = dialog_tx.clone();
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(&runtime);
        let surface = surface_source.clone();
        window.on_logging_dialog_browse(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if !file_dialogs_available() {
                let projection = runtime_ref.borrow_mut().set_logging_dialog_path_error(
                    "The system folder picker is unavailable here; type the folder path directly.",
                );
                apply_projection(&window, &projection);
                surface.refresh(&window);
                return;
            }
            let start_dir = window.get_logging_dialog_directory_text().to_string();
            spawn_pick_folder(
                dialog_tx.clone(),
                DialogKind::LoggingDirectory,
                "Choose the log folder",
                &start_dir,
            );
        });
    }

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_update_settings_scrollback_max_cells(move |value| {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .update_settings_scrollback_max_cells(value.as_ref());
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_reset_settings_terminal_defaults(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().reset_settings_terminal_defaults();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_settings_terminal(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_settings_terminal();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Save terminal error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_auth_method_agent(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_auth_method_agent();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_auth_method_password(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_auth_method_password();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_auth_method_keyboard_interactive(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_editor_auth_method_keyboard_interactive();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_auth_method_private_key(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_editor_auth_method_private_key();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_host_key_policy_strict(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().set_editor_host_key_policy_strict();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_host_key_policy_trust_on_first_use(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_editor_host_key_policy_trust_on_first_use();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_set_editor_host_key_policy_accept_any_for_testing(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .set_editor_host_key_policy_accept_any_for_testing();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_test_editor_auth(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().test_editor_auth();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_editor_to_saved_session(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_editor_to_saved_session();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Save editor error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_save_editor_and_connect(move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref.borrow_mut().save_editor_and_connect();
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => set_plain_status(&window, format!("Save+Connect error: {error}").into()),
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_sftp(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_sftp();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_tunnels(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_tunnels();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_toggle_commands(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().toggle_commands();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    // --- N3：面板停靠（布局由 Rust 计算；拖拽预览只更新投影，不刷新终端）------
    // 模板：`wire_panel!` 用于无失败路径的预览；`wire_panel_result!` 用于落盘操作。
    macro_rules! wire_panel {
        ($setter:ident, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {
            {
                let weak = window.as_weak();
                let runtime_ref = Rc::clone(&runtime);
                window.$setter(move |$($arg : $ty),*| {
                    if let Some(window) = weak.upgrade() {
                        let $runtime = &mut runtime_ref.borrow_mut();
                        let projection = $body;
                        apply_projection(&window, &projection);
                    }
                });
            }
        };
    }
    macro_rules! wire_panel_result {
        ($setter:ident, $error_prefix:expr, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {
            {
                let weak = window.as_weak();
                let runtime_ref = Rc::clone(&runtime);
                window.$setter(move |$($arg : $ty),*| {
                    let Some(window) = weak.upgrade() else {
                        return;
                    };
                    let result = {
                        let $runtime = &mut runtime_ref.borrow_mut();
                        $body
                    };
                    match result {
                        Ok(projection) => apply_projection(&window, &projection),
                        Err(error) => {
                            set_plain_status(&window, format!("{}: {error}", $error_prefix).into());
                        }
                    }
                });
            }
        };
    }

    // 内容区尺寸 → Rust 重新计算 px 几何。
    wire_panel!(on_panel_area_resized, |runtime, width: f32, height: f32| {
        runtime.set_panel_area_size(width, height)
    });

    // View/Panels 菜单：显示/隐藏面板。
    wire_panel_result!(
        on_toggle_panel,
        "Panel layout error",
        |runtime, id: SharedString| {
            match panel_id_from_str(id.as_ref()) {
                Some(panel) => runtime.toggle_panel_visible(panel),
                None => Ok(runtime.projection()),
            }
        }
    );

    // 面板 ⋯ 菜单动作。
    wire_panel_result!(
        on_panel_action,
        "Panel layout error",
        |runtime, id: SharedString, action: SharedString| {
            match panel_id_from_str(id.as_ref()) {
                None => Ok(runtime.projection()),
                Some(panel) => match action.as_ref() {
                    "move-left" => runtime.move_panel(panel, PanelSide::Left, None),
                    "move-right" => runtime.move_panel(panel, PanelSide::Right, None),
                    "collapse" => runtime.set_panel_collapsed(panel, true),
                    "expand" => runtime.set_panel_collapsed(panel, false),
                    "collapse-toggle" => runtime.toggle_panel_collapsed(panel),
                    "hide" => runtime.hide_panel(panel),
                    _ => Ok(runtime.projection()),
                },
            }
        }
    );

    // 窄窗：图标条展开 / 右栏 chevron。
    wire_panel_result!(
        on_panel_side_expand,
        "Panel layout error",
        |runtime, side: SharedString| {
            match panel_side_from_str(side.as_ref()) {
                Some(side) => runtime.expand_panel_side(side),
                None => Ok(runtime.projection()),
            }
        }
    );
    wire_panel_result!(
        on_panel_side_toggle,
        "Panel layout error",
        |runtime, side: SharedString| {
            match panel_side_from_str(side.as_ref()) {
                Some(side) => runtime.toggle_panel_side_expanded(side),
                None => Ok(runtime.projection()),
            }
        }
    );

    // 栏宽拖拽（预览不落盘，结束落盘）。
    wire_panel!(
        on_panel_width_preview,
        |runtime, side: SharedString, width: f32| {
            match panel_side_from_str(side.as_ref()) {
                Some(side) => runtime.preview_panel_side_width(side, width),
                None => runtime.projection(),
            }
        }
    );
    wire_panel_result!(
        on_panel_width_commit,
        "Panel layout save error",
        |runtime| runtime.commit_panel_layout()
    );

    // 分栏拖拽（px → 比例）。
    wire_panel!(
        on_panel_split_preview,
        |runtime, side: SharedString, boundary: i32, y: f32| {
            match panel_side_from_str(side.as_ref()) {
                Some(side) => runtime.preview_panel_split_pixels(
                    side,
                    usize::try_from(boundary).unwrap_or(usize::MAX),
                    y,
                ),
                None => runtime.projection(),
            }
        }
    );
    wire_panel_result!(
        on_panel_split_commit,
        "Panel layout save error",
        |runtime| runtime.commit_panel_layout()
    );

    // 面板头部拖拽（换边/换序 + 插入指示）。
    wire_panel!(
        on_panel_drag_start,
        |runtime, id: SharedString, x: f32, y: f32| {
            match panel_id_from_str(id.as_ref()) {
                Some(panel) => runtime.panel_drag_start(panel, x, y),
                None => runtime.projection(),
            }
        }
    );
    wire_panel!(
        on_panel_drag_move,
        |runtime, id: SharedString, x: f32, y: f32| {
            match panel_id_from_str(id.as_ref()) {
                Some(panel) => runtime.panel_drag_move(panel, x, y),
                None => runtime.projection(),
            }
        }
    );
    wire_panel_result!(
        on_panel_drag_drop,
        "Panel layout error",
        |runtime, id: SharedString, x: f32, y: f32| {
            match panel_id_from_str(id.as_ref()) {
                Some(panel) => runtime.panel_drag_drop(panel, x, y),
                None => Ok(runtime.projection()),
            }
        }
    );
    wire_panel!(on_panel_drag_cancel, |runtime| runtime.panel_drag_cancel());

    // N6：退出前强制停止所有活动日志并 flush（标签关闭路径在 runtime/tabs.rs）。
    let runtime_for_quit = Rc::clone(&runtime);
    window.on_quit_app(move || {
        if let Ok(mut runtime) = runtime_for_quit.try_borrow_mut() {
            runtime.stop_all_session_logging();
        }
        let _ = slint::quit_event_loop();
    });

    wire_n1_sftp_callbacks(window, &runtime, &surface_source, &clipboard, &dialog_tx);

    dialog_rx
}

/// N1 Phase 2：本地栏 / 双击多选 / 队列抽屉 / 拖动落点 / 剪贴板 / 冲突与属性。
#[allow(clippy::too_many_arguments)]
fn wire_n1_sftp_callbacks(
    window: &MainWindow,
    runtime: &Rc<RefCell<AppRuntime>>,
    surface_source: &TerminalSurface,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
    dialog_tx: &std::sync::mpsc::Sender<FileDialogOutcome>,
) {
    macro_rules! wire_projection {
        ($setter:ident, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {{
            let weak = window.as_weak();
            let runtime_ref = Rc::clone(runtime);
            let surface = surface_source.clone();
            window.$setter(move |$($arg : $ty),*| {
                if let Some(window) = weak.upgrade() {
                    let $runtime = &mut runtime_ref.borrow_mut();
                    let projection = $body;
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
            });
        }};
    }
    macro_rules! wire_result {
        ($setter:ident, $prefix:expr, |$runtime:ident $(, $arg:ident : $ty:ty)*| $body:expr) => {{
            let weak = window.as_weak();
            let runtime_ref = Rc::clone(runtime);
            let surface = surface_source.clone();
            window.$setter(move |$($arg : $ty),*| {
                let Some(window) = weak.upgrade() else {
                    return;
                };
                let result = {
                    let $runtime = &mut runtime_ref.borrow_mut();
                    $body
                };
                match result {
                    Ok(projection) => {
                        apply_projection(&window, &projection);
                        surface.refresh(&window);
                    }
                    Err(error) => {
                        set_plain_status(&window, format!("{}: {error}", $prefix).into());
                    }
                }
            });
        }};
    }

    // ---------------------------------------------------------- 本地栏
    wire_projection!(on_local_refresh, |runtime| runtime.refresh_local_pane());
    wire_projection!(on_local_up, |runtime| runtime.open_local_parent());
    wire_projection!(on_local_home, |runtime| runtime.open_local_home());
    wire_projection!(on_local_open_path, |runtime, path: SharedString| runtime
        .open_local_path(path.as_ref()));
    wire_projection!(on_local_sort_by, |runtime, column: SharedString| runtime
        .sort_local_by(column.as_ref()));
    wire_projection!(on_local_toggle_hidden, |runtime| runtime
        .toggle_local_hidden());
    wire_projection!(
        on_local_select_row,
        |runtime, index: i32, ctrl: bool, shift: bool| runtime.select_local_row(index, ctrl, shift)
    );
    wire_projection!(on_local_activate_row, |runtime, index: i32| runtime
        .activate_local_row(index));
    wire_projection!(on_local_toggle_collapsed, |runtime| runtime
        .toggle_local_collapsed());
    wire_projection!(on_local_retry, |runtime| runtime.retry_local_pane());

    // ---------------------------------------------------------- 远端多选
    wire_projection!(
        on_select_sftp_row,
        |runtime, index: i32, ctrl: bool, shift: bool| runtime.select_sftp_row(index, ctrl, shift)
    );

    // ---------------------------------------------------------- 队列抽屉
    wire_projection!(on_transfer_toggle_expanded, |runtime| runtime
        .toggle_transfer_drawer());
    wire_result!(
        on_transfer_pause,
        "SFTP transfer",
        |runtime, id: SharedString| runtime.pause_sftp_transfer(id.as_ref())
    );
    wire_result!(
        on_transfer_resume,
        "SFTP transfer",
        |runtime, id: SharedString| runtime.resume_sftp_transfer(id.as_ref())
    );
    wire_result!(
        on_transfer_retry,
        "SFTP transfer",
        |runtime, id: SharedString| runtime.retry_sftp_transfer(id.as_ref())
    );
    wire_result!(
        on_transfer_cancel,
        "SFTP transfer",
        |runtime, id: SharedString| runtime.cancel_sftp_transfer(id.as_ref())
    );
    wire_result!(
        on_transfer_remove,
        "SFTP transfer",
        |runtime, id: SharedString| runtime.remove_sftp_transfer(id.as_ref())
    );
    wire_result!(on_transfer_clear_completed, "SFTP transfer", |runtime| {
        runtime.clear_completed_sftp_transfers()
    });

    // ---------------------------------------------------------- 剪贴板
    // Ctrl+C/X 取"有选择的栏"（远端优先）；Ctrl+V 总是跨栏粘贴。
    wire_projection!(on_copy_file_selection, |runtime| runtime
        .copy_selection_to_clipboard());
    wire_projection!(on_cut_file_selection, |runtime| runtime
        .cut_selection_to_clipboard());
    wire_projection!(on_paste_cross_pane, |runtime| runtime.paste_cross_pane());

    // ---------------------------------------------------------- 弹窗
    wire_result!(
        on_sftp_conflict_resolve,
        "SFTP conflict",
        |runtime, policy: SharedString| runtime.resolve_sftp_conflict(policy.as_ref())
    );
    wire_projection!(on_sftp_conflict_cancel, |runtime| runtime
        .cancel_sftp_conflict());
    wire_projection!(on_sftp_properties_close, |runtime| runtime
        .close_sftp_properties());

    {
        // Properties → Copy Path：写系统剪贴板。
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(runtime);
        let clipboard = Rc::clone(clipboard);
        window.on_sftp_properties_copy_path(move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let path = runtime_ref.borrow().sftp_selected_path_text();
            copy_text_to_clipboard(&clipboard, &path);
            set_plain_status(&window, format!("Copied `{path}`.").into());
        });
    }

    // ---------------------------------------------------------- 拖动载荷
    {
        // `key` 只作为 Slint 端 data 绑定的依赖（选择变化时重新求值）；载荷在
        // 这里按当前选择重新构建。
        let runtime_ref = Rc::clone(runtime);
        window.on_drag_local_transfer(move |_key: SharedString| {
            build_drag_transfer(&runtime_ref.borrow(), crate::runtime::ClipboardSide::Local)
        });
        let runtime_ref = Rc::clone(runtime);
        window.on_drag_remote_transfer(move |_key: SharedString| {
            build_drag_transfer(&runtime_ref.borrow(), crate::runtime::ClipboardSide::Remote)
        });
    }

    // ---------------------------------------------------------- 落点
    wire_result!(
        on_drop_on_remote_row,
        "SFTP drop",
        |runtime, index: i32, data: slint::DataTransfer, action: DragAction| {
            let target = remote_drop_target(runtime, index);
            handle_remote_drop(runtime, &data, action, &target)
        }
    );
    wire_result!(
        on_drop_on_remote_blank,
        "SFTP drop",
        |runtime, data: slint::DataTransfer, action: DragAction| {
            let target = runtime.sftp_path.clone();
            handle_remote_drop(runtime, &data, action, &target)
        }
    );
    wire_result!(
        on_drop_on_local,
        "SFTP drop",
        |runtime, data: slint::DataTransfer, action: DragAction| {
            handle_local_drop(runtime, &data, action)
        }
    );

    // ---------------------------------------------------------- 菜单动作
    {
        let weak = window.as_weak();
        let runtime_ref = Rc::clone(runtime);
        let surface = surface_source.clone();
        let clipboard = Rc::clone(clipboard);
        let dialog_tx = dialog_tx.clone();
        window.on_sftp_menu_action(move |action: SharedString| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let result = {
                let mut runtime = runtime_ref.borrow_mut();
                dispatch_sftp_menu_action(
                    &window,
                    &mut runtime,
                    &clipboard,
                    &dialog_tx,
                    action.as_ref(),
                )
            };
            match result {
                Ok(projection) => {
                    apply_projection(&window, &projection);
                    surface.refresh(&window);
                }
                Err(error) => {
                    set_plain_status(&window, format!("SFTP: {error}").into());
                }
            }
        });
    }

    // ---------------------------------------------------------- 路径回退输入
    wire_result!(
        on_upload_local_path_to_remote,
        "SFTP upload",
        |runtime, path: SharedString| runtime.upload_local_path_to_remote(path.as_ref(), "ask")
    );
    wire_result!(
        on_download_sftp_selection_to,
        "SFTP download",
        |runtime, path: SharedString| runtime.download_sftp_selection_to(path.as_ref(), "ask")
    );
    wire_result!(on_delete_sftp_selection, "SFTP delete", |runtime| runtime
        .delete_sftp_selection());
    wire_result!(
        on_chmod_sftp_selection,
        "SFTP chmod",
        |runtime, permissions: SharedString| runtime.chmod_sftp_selection(permissions.as_ref())
    );
    wire_result!(
        on_copy_sftp_selection_to,
        "SFTP copy",
        |runtime, path: SharedString| runtime.copy_sftp_selection_to(path.as_ref(), "ask")
    );
}

/// N4：rfd 文件对话框的结果（worker 线程 → mpsc → UI 定时器 drain）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FileDialogOutcome {
    /// `pick_file` 返回了一个路径。
    Picked { kind: DialogKind, path: String },
    /// `save_file` 写盘结果（`error` 为空 = 成功）。
    Saved {
        kind: DialogKind,
        path: String,
        error: Option<String>,
    },
}

/// N4：对话框用途（决定结果回填到哪个页面状态）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogKind {
    PrivateKeyImport,
    AuthKeyFile,
    PublicKeyExport,
    HostKeysExport,
    /// N6：终端日志弹窗的保存位置（pick_folder）。
    LoggingDirectory,
    /// N1：SFTP 上传文件（pick_file）→ 上传到当前/选中远端目录。
    SftpUploadFile,
    /// N1：SFTP 下载目标目录（pick_folder）→ 下载当前远端选择。
    SftpDownloadFolder,
}

/// rfd 的 GTK 后端是单个全局线程：并发/重复调用会在同一 GTK 主循环上嵌套对话框。
static DIALOG_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// N6：rfd 需要显示服务器；纯无头环境下用 display 预检降级为路径输入
/// （gtk3 不能用 `pick_*` 返回 `None` 判不可用——取消也是 `None`，见 spike §10.3）。
fn file_dialogs_available() -> bool {
    std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

/// N6：在 worker 线程上打开 `pick_folder`（禁止 UI 线程直调：实测会冻结渲染）。
fn spawn_pick_folder(
    dialog_tx: std::sync::mpsc::Sender<FileDialogOutcome>,
    kind: DialogKind,
    title: &str,
    start_dir: &str,
) {
    if DIALOG_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = title.to_owned();
    let start_dir = start_dir.to_owned();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title(title);
        if !start_dir.is_empty() {
            dialog = dialog.set_directory(start_dir);
        }
        let picked = dialog.pick_folder();
        DIALOG_IN_FLIGHT.store(false, Ordering::SeqCst);
        if let Some(path) = picked {
            let _ = dialog_tx.send(FileDialogOutcome::Picked {
                kind,
                path: path.display().to_string(),
            });
        }
    });
}

/// N6：用系统默认程序打开文件/目录（worker 线程；失败静默——菜单项已按存在性启用）。
fn spawn_open_path(path: String) {
    if path.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        let program = "open";
        #[cfg(target_os = "windows")]
        let program = "explorer";
        #[cfg(all(unix, not(target_os = "macos")))]
        let program = "xdg-open";
        let _ = std::process::Command::new(program)
            .arg(&path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    });
}

/// N4：在 worker 线程上打开 `pick_file`（禁止 UI 线程直调：实测会冻结渲染）。
fn spawn_pick_file(
    dialog_tx: std::sync::mpsc::Sender<FileDialogOutcome>,
    kind: DialogKind,
    title: &str,
) {
    if DIALOG_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = title.to_owned();
    std::thread::spawn(move || {
        let picked = rfd::FileDialog::new().set_title(title).pick_file();
        DIALOG_IN_FLIGHT.store(false, Ordering::SeqCst);
        if let Some(path) = picked {
            let _ = dialog_tx.send(FileDialogOutcome::Picked {
                kind,
                path: path.display().to_string(),
            });
        }
    });
}

/// N4：在 worker 线程上打开 `save_file` 并写文本（结果经 mpsc 回填状态）。
fn spawn_save_text(
    dialog_tx: std::sync::mpsc::Sender<FileDialogOutcome>,
    kind: DialogKind,
    title: &str,
    file_name: &str,
    text: String,
) {
    if DIALOG_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return;
    }
    let title = title.to_owned();
    let file_name = file_name.to_owned();
    std::thread::spawn(move || {
        let picked = rfd::FileDialog::new()
            .set_title(title)
            .set_file_name(file_name)
            .save_file();
        DIALOG_IN_FLIGHT.store(false, Ordering::SeqCst);
        let Some(path) = picked else {
            return;
        };
        let error = std::fs::write(&path, text.as_bytes())
            .err()
            .map(|error| error.to_string());
        let _ = dialog_tx.send(FileDialogOutcome::Saved {
            kind,
            path: path.display().to_string(),
            error,
        });
    });
}

// ---------------------------------------------------------------------------
// N1 Phase 2：SFTP 拖动载荷、落点与上下文菜单动作
// ---------------------------------------------------------------------------

/// In-app drag payload (process-local `user_data`; OS interoperability is not
/// promised by the design).
#[derive(Clone)]
struct SftpDragPayload {
    side: crate::runtime::ClipboardSide,
    paths: Vec<PathBuf>,
}

/// Builds the `data-transfer` for a drag that starts in the given pane.
fn build_drag_transfer(
    runtime: &AppRuntime,
    side: crate::runtime::ClipboardSide,
) -> slint::DataTransfer {
    let paths = match side {
        crate::runtime::ClipboardSide::Local => runtime.local_selected_paths(),
        crate::runtime::ClipboardSide::Remote => runtime
            .sftp_selected_paths()
            .into_iter()
            .map(PathBuf::from)
            .collect(),
    };
    let mut transfer = slint::DataTransfer::default();
    transfer.set_user_data(Rc::new(SftpDragPayload { side, paths }));
    transfer
}

fn drag_payload(data: &slint::DataTransfer) -> Option<SftpDragPayload> {
    data.user_data()
        .and_then(|value| value.downcast::<SftpDragPayload>().ok())
        .map(|value| (*value).clone())
}

fn external_drop_paths(data: &slint::DataTransfer) -> Vec<PathBuf> {
    data.file_paths()
        .map(|paths| paths.map(PathBuf::from).collect())
        .unwrap_or_default()
}

/// The directory a drop on visible row `index` targets (folder rows use their
/// own path, files fall back to the current remote directory).
fn remote_drop_target(runtime: &AppRuntime, index: i32) -> String {
    runtime
        .sftp_visible_entry_at(index)
        .filter(|entry| matches!(entry.kind, yshell_sftp::FsEntryKind::Directory))
        .map(|entry| entry.path)
        .unwrap_or_else(|| runtime.sftp_path.clone())
}

fn handle_remote_drop(
    runtime: &mut AppRuntime,
    data: &slint::DataTransfer,
    action: DragAction,
    target_dir: &str,
) -> AppResult<AppProjection> {
    let move_source = action == DragAction::Move;
    match drag_payload(data) {
        Some(payload) => match payload.side {
            crate::runtime::ClipboardSide::Local => {
                // 本地 → 远端：默认复制，Ctrl（协商为 Move）= 移动。
                runtime.drop_local_paths_on_remote(&payload.paths, target_dir, move_source)
            }
            crate::runtime::ClipboardSide::Remote => {
                // 远端内拖动 = 移动到目录（设计 §2），与修饰键无关。
                let entries = runtime.clipboard_remote_entries(&payload.paths);
                runtime.drop_remote_entries_on_remote(&entries, target_dir, true)
            }
        },
        None => {
            let paths = external_drop_paths(data);
            runtime.drop_external_paths_on_remote(&paths, target_dir)
        }
    }
}

fn handle_local_drop(
    runtime: &mut AppRuntime,
    data: &slint::DataTransfer,
    action: DragAction,
) -> AppResult<AppProjection> {
    let move_source = action == DragAction::Move;
    match drag_payload(data) {
        Some(payload) => match payload.side {
            crate::runtime::ClipboardSide::Local => {
                Ok(runtime.drop_local_paths_on_local(&payload.paths, move_source))
            }
            crate::runtime::ClipboardSide::Remote => {
                let entries = runtime.clipboard_remote_entries(&payload.paths);
                runtime.drop_remote_entries_on_local(&entries, move_source)
            }
        },
        None => {
            let paths = external_drop_paths(data);
            Ok(runtime.drop_local_paths_on_local(&paths, false))
        }
    }
}

fn copy_text_to_clipboard(clipboard: &Rc<RefCell<Option<ClipboardContext>>>, text: &str) {
    if let Some(context) = clipboard.borrow_mut().as_mut() {
        let _ = context.set_contents(text.to_owned());
    }
}

/// Dispatches the SFTP context menu's semantic action ids.
fn dispatch_sftp_menu_action(
    window: &MainWindow,
    runtime: &mut AppRuntime,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
    dialog_tx: &std::sync::mpsc::Sender<FileDialogOutcome>,
    action: &str,
) -> AppResult<AppProjection> {
    let local_scope = window.get_sftp_menu_target_kind_text().starts_with("local");
    let local_dir = runtime.local_pane.dir.display().to_string();
    match action {
        "open" => runtime.activate_sftp_entry(),
        "edit" => runtime.edit_sftp_selected(),
        "download" | "download-selection" => runtime.download_sftp_selection_to(&local_dir, "ask"),
        "download-to" => {
            if file_dialogs_available() {
                spawn_pick_folder(
                    dialog_tx.clone(),
                    DialogKind::SftpDownloadFolder,
                    "Choose download folder",
                    &local_dir,
                );
                Ok(runtime.projection())
            } else {
                window.set_sftp_dialog_local_text(local_dir.clone().into());
                window.set_sftp_dialog_kind("download".into());
                Ok(runtime.projection())
            }
        }
        "upload" | "upload-selection" => {
            if local_scope || action == "upload-selection" {
                runtime.upload_local_selection_to_remote("ask")
            } else if file_dialogs_available() {
                spawn_pick_file(
                    dialog_tx.clone(),
                    DialogKind::SftpUploadFile,
                    "Upload to remote",
                );
                Ok(runtime.projection())
            } else {
                window.set_sftp_dialog_local_text("".into());
                window.set_sftp_dialog_kind("upload".into());
                Ok(runtime.projection())
            }
        }
        "upload-here" => {
            runtime.sftp_upload_dir_override = runtime.sftp_selected_directory();
            if file_dialogs_available() {
                spawn_pick_file(
                    dialog_tx.clone(),
                    DialogKind::SftpUploadFile,
                    "Upload into the selected remote folder",
                );
                Ok(runtime.projection())
            } else {
                window.set_sftp_dialog_local_text("".into());
                window.set_sftp_dialog_kind("upload".into());
                Ok(runtime.projection())
            }
        }
        "new-folder" => {
            window.set_sftp_dialog_name_text("".into());
            window.set_sftp_dialog_kind("new-folder".into());
            Ok(runtime.projection())
        }
        "rename" => {
            let name = runtime.sftp_selected_name_text();
            window.set_sftp_dialog_name_text(name.into());
            window.set_sftp_dialog_kind("rename".into());
            Ok(runtime.projection())
        }
        "delete" | "delete-selection" => {
            window.set_sftp_dialog_kind("delete".into());
            Ok(runtime.projection())
        }
        "chmod" | "chmod-selection" => {
            let permissions = runtime.sftp_selected_permissions_text();
            window.set_sftp_dialog_permissions_text(permissions.into());
            window.set_sftp_dialog_kind("chmod".into());
            Ok(runtime.projection())
        }
        "copy-to" => {
            window.set_sftp_dialog_local_text(runtime.sftp_path.clone().into());
            window.set_sftp_dialog_kind("copy-to".into());
            Ok(runtime.projection())
        }
        "properties" => Ok(runtime.open_sftp_properties()),
        // 本地菜单 "Move to Remote"：上传成功后删除本地源（与 Ctrl 拖动同一路径）。
        "move-selection" => runtime.move_local_selection_to_remote(),
        "copy" => Ok(if local_scope {
            runtime.copy_local_selection_to_clipboard()
        } else {
            runtime.copy_remote_selection_to_clipboard()
        }),
        "copy-local" => Ok(runtime.copy_local_selection_to_clipboard()),
        "copy-path" => {
            let text = if local_scope {
                runtime
                    .local_selected_paths()
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                runtime.sftp_selected_paths().join("\n")
            };
            copy_text_to_clipboard(clipboard, &text);
            runtime.status_text = format!(
                "Copied {} path(s).",
                if text.is_empty() {
                    0
                } else {
                    text.lines().count()
                }
            );
            Ok(runtime.projection())
        }
        "copy-local-path" => {
            let text = runtime
                .local_selected_paths()
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            copy_text_to_clipboard(clipboard, &text);
            runtime.status_text = "Copied local path(s).".to_owned();
            Ok(runtime.projection())
        }
        "copy-path-current" => {
            copy_text_to_clipboard(clipboard, &runtime.sftp_path);
            runtime.status_text = format!("Copied `{}`.", runtime.sftp_path);
            Ok(runtime.projection())
        }
        "paste-into-remote" => Ok(runtime.paste_file_clipboard_into_remote()),
        "paste-into-local" => Ok(runtime.paste_file_clipboard_into_local()),
        "refresh" => {
            if local_scope {
                Ok(runtime.refresh_local_pane())
            } else {
                runtime.refresh_active_sftp_listing()
            }
        }
        "local-refresh" => Ok(runtime.refresh_local_pane()),
        "toggle-hidden" => {
            if local_scope {
                Ok(runtime.toggle_local_hidden())
            } else {
                Ok(runtime.toggle_sftp_hidden_files())
            }
        }
        "local-toggle-hidden" => Ok(runtime.toggle_local_hidden()),
        "up" => runtime.open_sftp_parent(),
        "home" => runtime.open_sftp_path("/"),
        unknown => {
            runtime.status_text = format!("Unknown SFTP menu action `{unknown}`.");
            Ok(runtime.projection())
        }
    }
}

/// N4：把对话框结果回填到运行时（在 UI 线程的定时器里调用）。
fn apply_dialog_outcome(runtime: &mut AppRuntime, outcome: FileDialogOutcome) -> AppProjection {
    match outcome {
        FileDialogOutcome::Picked { kind, path } => match kind {
            DialogKind::PrivateKeyImport => runtime.update_private_keys_import_path(&path),
            DialogKind::AuthKeyFile => runtime.set_auth_prompt_key_path(&path),
            // N6：终端日志弹窗的保存位置。
            DialogKind::LoggingDirectory => runtime.update_logging_dialog_directory(&path),
            // N1：rfd 直接触发的 SFTP 上传/下载（错误 → 状态栏）。
            DialogKind::SftpUploadFile => runtime
                .upload_local_path_to_remote(&path, "ask")
                .unwrap_or_else(|error| {
                    runtime.status_text = format!("SFTP upload: {error}");
                    runtime.projection()
                }),
            DialogKind::SftpDownloadFolder => runtime
                .download_sftp_selection_to(&path, "ask")
                .unwrap_or_else(|error| {
                    runtime.status_text = format!("SFTP download: {error}");
                    runtime.projection()
                }),
            // 保存类对话框不会产生 Picked。
            DialogKind::PublicKeyExport | DialogKind::HostKeysExport => runtime.projection(),
        },
        FileDialogOutcome::Saved { kind, path, error } => {
            let detail = match (&error, kind) {
                (Some(error), DialogKind::PublicKeyExport) => format!("Export failed: {error}"),
                (Some(error), DialogKind::HostKeysExport) => format!("Export failed: {error}"),
                (Some(error), _) => format!("Dialog failed: {error}"),
                (None, DialogKind::PublicKeyExport) => {
                    format!("Exported the public key to `{path}`.")
                }
                (None, DialogKind::HostKeysExport) => {
                    format!("Exported host keys to `{path}`.")
                }
                (None, _) => format!("Saved to `{path}`."),
            };
            match kind {
                DialogKind::HostKeysExport => runtime.set_host_keys_status(&detail),
                DialogKind::LoggingDirectory => runtime.set_logging_dialog_path_error(&detail),
                _ => runtime.set_private_keys_status(&detail),
            }
        }
    }
}

fn start_terminal_poll_timer(
    window: &MainWindow,
    runtime: Rc<RefCell<AppRuntime>>,
    surface: TerminalSurface,
    dialog_rx: std::sync::mpsc::Receiver<FileDialogOutcome>,
    sftp_job_rx: std::sync::mpsc::Receiver<crate::sftp_jobs::SftpJobMessage>,
) -> Timer {
    let timer = Timer::default();
    let weak = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_millis(120), move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        // N4：rfd 对话框结果回填（worker 线程 → mpsc → 本次 drain）。
        {
            let Ok(mut runtime) = runtime.try_borrow_mut() else {
                return;
            };
            let mut last_projection = None;
            while let Ok(outcome) = dialog_rx.try_recv() {
                last_projection = Some(apply_dialog_outcome(&mut runtime, outcome));
            }
            // N1：SFTP 传输 worker 消息（进度/完成/冲突）。
            while let Ok(message) = sftp_job_rx.try_recv() {
                last_projection = Some(runtime.apply_sftp_job_message(message));
            }
            if let Some(projection) = last_projection {
                apply_projection(&window, &projection);
            }
        }
        surface.sync_scale_factor(&window);
        let (columns, rows) = terminal_size_from_viewport(
            window.get_terminal_viewport_width_px(),
            window.get_terminal_viewport_height_px(),
            surface.cell_size(),
            surface.scale_factor(),
        );
        let projection = {
            let Ok(mut runtime) = runtime.try_borrow_mut() else {
                return;
            };
            let mut projection = None;
            match runtime.sync_active_terminal_size_passive(columns, rows) {
                Ok(Some(updated)) => projection = Some(updated),
                Ok(None) => {}
                Err(error) => {
                    set_plain_status(&window, format!("Terminal resize error: {error}").into())
                }
            }
            match runtime.poll_all_terminal_outputs() {
                Ok(Some(updated)) => projection = Some(updated),
                Ok(None) => {}
                Err(error) => {
                    set_plain_status(&window, format!("Terminal poll error: {error}").into())
                }
            }
            projection
        };
        if let Some(projection) = projection {
            apply_projection(&window, &projection);
        }
        surface.refresh(&window);
    });
    timer
}

/// Coerce a window scale factor into a usable value.
fn sanitize_scale_factor(scale_factor: f32) -> f32 {
    if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    }
}

/// Map a logical-pixel viewport onto terminal columns/rows.
///
/// `cell_size` is in **physical** pixels (the renderer rasterizes at
/// `font_size * scale_factor`), while Slint reports the viewport in logical
/// pixels, so the viewport is scaled up first.
///
/// The grid rounds **up** (`ceil`): the bitmap is then at least as large as the
/// viewport and is displayed 1:1 in physical pixels, anchored at the bitmap
/// origin; the overflowing last column/row is clipped by the terminal surface
/// (a partially visible cell instead of a stretched bitmap or a blank strip).
fn terminal_size_from_viewport(
    width_px: f32,
    height_px: f32,
    cell_size: (u32, u32),
    scale_factor: f32,
) -> (u16, u16) {
    const MIN_COLUMNS: u16 = 20;
    const MIN_ROWS: u16 = 4;

    let scale_factor = sanitize_scale_factor(scale_factor);
    let cell_width = cell_size.0.max(1) as f32;
    let cell_height = cell_size.1.max(1) as f32;
    let physical_width = width_px.max(0.0) * scale_factor;
    let physical_height = height_px.max(0.0) * scale_factor;
    let columns = ((physical_width / cell_width).ceil() as u16).max(MIN_COLUMNS);
    let rows = ((physical_height / cell_height).ceil() as u16).max(MIN_ROWS);
    (columns, rows)
}

/// Convert a pixel position on the terminal surface into a grid cell plus the
/// absolute scrollback-aware line.
///
/// `x`/`y` are logical pixels relative to the bitmap origin (the top-left of
/// the terminal image), exactly what `terminal_view.slint` forwards from its
/// `TouchArea`.
fn terminal_grid_point(
    surface: &TerminalSurface,
    runtime: &Rc<RefCell<AppRuntime>>,
    x: f32,
    y: f32,
) -> Option<(u16, u16)> {
    let metrics = runtime
        .try_borrow()
        .ok()
        .and_then(|runtime| runtime.active_terminal_viewport_metrics())?;
    let (column, viewport_row) = terminal_grid_cell(
        x,
        y,
        surface.cell_size(),
        surface.scale_factor(),
        metrics.columns,
        metrics.rows,
    )?;
    let absolute_row = metrics.top_absolute_row + viewport_row as usize;
    Some((column, u16::try_from(absolute_row).ok()?))
}

/// Physical-pixel mapping of a logical position onto the terminal grid.
///
/// The bitmap and the cell metrics are physical; the pointer is scaled up by the
/// window scale factor before dividing by the physical cell size, then clamped to
/// the grid.
fn terminal_grid_cell(
    x: f32,
    y: f32,
    cell_size: (u32, u32),
    scale_factor: f32,
    columns: u16,
    rows: u16,
) -> Option<(u16, u16)> {
    if columns == 0 || rows == 0 {
        return None;
    }
    let scale_factor = sanitize_scale_factor(scale_factor);
    let cell_width = cell_size.0.max(1) as f32;
    let cell_height = cell_size.1.max(1) as f32;
    let column =
        ((x.max(0.0) * scale_factor / cell_width).floor() as u32).min(u32::from(columns - 1));
    let row = ((y.max(0.0) * scale_factor / cell_height).floor() as u32).min(u32::from(rows - 1));
    Some((column as u16, row as u16))
}

/// Copy the active terminal selection into the OS clipboard.
fn copy_selection_to_clipboard(
    runtime: &Rc<RefCell<AppRuntime>>,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
) -> AppResult<AppProjection> {
    let projection = runtime.borrow_mut().copy_active_terminal_selection()?;
    if let Some(clipboard) = clipboard.borrow_mut().as_mut() {
        let _ = clipboard.set_contents(runtime.borrow().terminal_clipboard_text().to_owned());
    }
    Ok(projection)
}

/// Paste the OS clipboard into the active terminal (falls back to the app
/// clipboard buffer when the OS clipboard is unavailable).
fn paste_clipboard_into_terminal(
    runtime: &Rc<RefCell<AppRuntime>>,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
) -> AppResult<AppProjection> {
    let clipboard_text = clipboard
        .borrow_mut()
        .as_mut()
        .and_then(|clipboard| clipboard.get_contents().ok());
    match clipboard_text {
        Some(text) => runtime.borrow_mut().paste_text_into_terminal(&text),
        None => runtime.borrow_mut().paste_terminal_clipboard(),
    }
}

/// A key press from the terminal surface.
struct TerminalKeyPress<'a> {
    text: &'a str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
}

/// W5-A2：把一个按键转发给窗口级的快捷键分发函数
/// （`MainWindow.dispatch-global-shortcut`，与窗口根 FocusScope 共用同一份映射）。
///
/// 返回 `true` 表示该组合被窗口级快捷键消费，调用方不得再把它编码成终端输入；
/// 未匹配的组合返回 `false`，按键继续走终端语义（例如 Ctrl+C 中断、Ctrl+L 清屏）。
fn dispatch_global_shortcut(
    window: &MainWindow,
    text: &str,
    ctrl: bool,
    shift: bool,
    alt: bool,
    meta: bool,
) -> bool {
    window.invoke_dispatch_global_shortcut(text.into(), ctrl, shift, alt, meta)
}

enum ScrollRequest {
    Lines(i32),
    Bottom,
}

/// Route a key press from the terminal surface: copy/paste and scrollback
/// shortcuts first, then the VT key encoding. The returned projection must be
/// applied to the window so scroll indicators and status text stay in sync.
fn handle_terminal_key(
    runtime: &Rc<RefCell<AppRuntime>>,
    clipboard: &Rc<RefCell<Option<ClipboardContext>>>,
    key: TerminalKeyPress<'_>,
) -> AppResult<AppProjection> {
    let TerminalKeyPress {
        text,
        ctrl,
        alt,
        shift,
        meta,
    } = key;
    let lowered = text.to_lowercase();
    if ctrl && shift && lowered == "c" {
        return copy_selection_to_clipboard(runtime, clipboard);
    }
    if ctrl && !shift && (lowered == "c" || text == "\u{3}") {
        if runtime.borrow().active_terminal_selection_active() {
            return copy_selection_to_clipboard(runtime, clipboard);
        }
        return runtime
            .borrow_mut()
            .send_active_terminal_key("\u{3}", false, false, false, false);
    }
    if ctrl && lowered == "v" || shift && text == "\u{F727}" {
        return paste_clipboard_into_terminal(runtime, clipboard);
    }
    if shift {
        let page = runtime
            .borrow()
            .active_terminal_viewport_metrics()
            .map(|metrics| i32::from(metrics.rows.saturating_sub(1)).max(1))
            .unwrap_or(10);
        let scroll = match text {
            "\u{F72C}" => Some(ScrollRequest::Lines(page)),
            "\u{F72D}" => Some(ScrollRequest::Lines(-page)),
            "\u{F729}" => Some(ScrollRequest::Lines(i32::MAX)),
            "\u{F72B}" => Some(ScrollRequest::Bottom),
            _ => None,
        };
        if let Some(request) = scroll {
            return match request {
                ScrollRequest::Lines(delta) => runtime.borrow_mut().scroll_active_terminal(delta),
                ScrollRequest::Bottom => runtime.borrow_mut().scroll_active_terminal_to_bottom(),
            };
        }
    }
    runtime
        .borrow_mut()
        .send_active_terminal_key(text, ctrl, alt, shift, meta)
}

fn local_row_from(row: &crate::local_fs::LocalRowData) -> LocalRow {
    LocalRow {
        name: row.name.clone().into(),
        path_text: row.path_text.clone().into(),
        kind_text: row.kind_text.clone().into(),
        size_text: row.size_text.clone().into(),
        modified_text: row.modified_text.clone().into(),
        permissions_text: row.permissions_text.clone().into(),
        is_dir: row.is_dir,
        is_symlink: row.is_symlink,
        is_parent: row.is_parent,
        selected: row.selected,
    }
}

fn local_row_matches(current: &LocalRow, next: &crate::local_fs::LocalRowData) -> bool {
    current.name.as_str() == next.name
        && current.path_text.as_str() == next.path_text
        && current.size_text.as_str() == next.size_text
        && current.modified_text.as_str() == next.modified_text
        && current.permissions_text.as_str() == next.permissions_text
        && current.is_dir == next.is_dir
        && current.is_symlink == next.is_symlink
        && current.is_parent == next.is_parent
        && current.selected == next.selected
}

fn set_local_rows_if_changed(window: &MainWindow, rows: &[crate::local_fs::LocalRowData]) {
    let current = window.get_local_rows();
    if current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows.iter())
            .all(|(current, next)| local_row_matches(&current, next))
    {
        return;
    }
    window.set_local_rows(ModelRc::new(VecModel::from(
        rows.iter().map(local_row_from).collect::<Vec<_>>(),
    )));
}

fn transfer_row_from(row: &TransferRowData) -> SftpTransferRow {
    SftpTransferRow {
        id: row.id.clone().into(),
        direction_id: row.direction_id.clone().into(),
        direction_text: row.direction_text.clone().into(),
        file_name_text: row.file_name_text.clone().into(),
        source_text: row.source_text.clone().into(),
        destination_text: row.destination_text.clone().into(),
        status_id: row.status_id.clone().into(),
        status_text: row.status_text.clone().into(),
        progress_text: row.progress_text.clone().into(),
        progress_percent: row.progress_percent,
        error_text: row.error_text.clone().into(),
        retry_count: row.retry_count,
        max_retries: row.max_retries,
        can_pause: row.can_pause,
        can_resume: row.can_resume,
        can_retry: row.can_retry,
        can_cancel: row.can_cancel,
        can_remove: row.can_remove,
        is_active: row.is_active,
        is_failed: row.is_failed,
        is_completed: row.is_completed,
    }
}

fn transfer_row_matches(current: &SftpTransferRow, next: &TransferRowData) -> bool {
    current.id.as_str() == next.id
        && current.status_id.as_str() == next.status_id
        && current.progress_text.as_str() == next.progress_text
        && current.file_name_text.as_str() == next.file_name_text
        && current.error_text.as_str() == next.error_text
        && current.can_pause == next.can_pause
        && current.can_resume == next.can_resume
        && current.can_retry == next.can_retry
        && current.can_cancel == next.can_cancel
        && current.can_remove == next.can_remove
}

fn set_transfer_rows_if_changed(window: &MainWindow, rows: &[TransferRowData]) {
    let current = window.get_transfer_rows();
    if current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows.iter())
            .all(|(current, next)| transfer_row_matches(&current, next))
    {
        return;
    }
    window.set_transfer_rows(ModelRc::new(VecModel::from(
        rows.iter().map(transfer_row_from).collect::<Vec<_>>(),
    )));
}

fn sftp_row_from(row: &SftpRowData) -> SftpRow {
    SftpRow {
        name: row.name.clone().into(),
        path_text: row.path_text.clone().into(),
        kind_text: row.kind_text.clone().into(),
        size_text: row.size_text.clone().into(),
        modified_text: row.modified_text.clone().into(),
        permissions_text: row.permissions_text.clone().into(),
        is_dir: row.is_dir,
        is_symlink: row.is_symlink,
        selected: row.selected,
    }
}

fn sftp_crumb_from(crumb: &SftpCrumbData) -> SftpCrumb {
    SftpCrumb {
        label: crumb.label.clone().into(),
        path: crumb.path.clone().into(),
    }
}

fn sftp_row_matches(current: &SftpRow, next: &SftpRowData) -> bool {
    current.name.as_str() == next.name
        && current.kind_text.as_str() == next.kind_text
        && current.size_text.as_str() == next.size_text
        && current.modified_text.as_str() == next.modified_text
        && current.permissions_text.as_str() == next.permissions_text
        && current.is_dir == next.is_dir
        && current.is_symlink == next.is_symlink
        && current.selected == next.selected
}

fn sftp_crumb_matches(current: &SftpCrumb, next: &SftpCrumbData) -> bool {
    current.label.as_str() == next.label && current.path.as_str() == next.path
}

/// Replaces the row model only when the projected rows changed, so that
/// selecting/sorting-adjacent updates do not recreate every list row instance
/// (which would break click counting and reset the ListView scroll state).
fn set_sftp_rows_if_changed(window: &MainWindow, rows: &[SftpRowData]) {
    let current = window.get_sftp_rows();
    let unchanged = current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows)
            .all(|(current, next)| sftp_row_matches(&current, next));
    if unchanged {
        return;
    }
    window.set_sftp_rows(ModelRc::new(VecModel::from(
        rows.iter().map(sftp_row_from).collect::<Vec<_>>(),
    )));
}

fn set_sftp_crumbs_if_changed(window: &MainWindow, crumbs: &[SftpCrumbData]) {
    let current = window.get_sftp_crumbs();
    let unchanged = current.row_count() == crumbs.len()
        && current
            .iter()
            .zip(crumbs)
            .all(|(current, next)| sftp_crumb_matches(&current, next));
    if unchanged {
        return;
    }
    window.set_sftp_crumbs(ModelRc::new(VecModel::from(
        crumbs.iter().map(sftp_crumb_from).collect::<Vec<_>>(),
    )));
}

fn workspace_tab_from(tab: &TabData) -> WorkspaceTab {
    WorkspaceTab {
        id: tab.id.clone().into(),
        title: tab.title.clone().into(),
        state_text: tab.state_text.clone().into(),
        connected: tab.connected,
        active: tab.active,
        kind_text: tab.kind_text.clone().into(),
        logging: tab.logging,
    }
}

fn workspace_tab_matches(current: &WorkspaceTab, next: &TabData) -> bool {
    current.id.as_str() == next.id
        && current.title.as_str() == next.title
        && current.state_text.as_str() == next.state_text
        && current.connected == next.connected
        && current.active == next.active
        && current.kind_text.as_str() == next.kind_text
        && current.logging == next.logging
}

/// Replaces the tab model only when the projected tabs changed, so that a
/// background poll does not recreate every tab instance (which would reset
/// click counting and the Flickable scroll position, same rationale as the
/// SFTP rows). When the count is unchanged, equal-length updates are applied
/// in place for the same reason (switching tabs only flips `active`).
fn set_tabs_if_changed(window: &MainWindow, tabs: &[TabData]) {
    let current = window.get_tabs();
    if tabs.is_empty() && current.row_count() == 0 {
        return;
    }
    if current.row_count() == tabs.len() {
        let handle: Option<std::rc::Rc<dyn Model<Data = WorkspaceTab>>> =
            current.clone().try_into().ok();
        if let Some(handle) = handle {
            if let Some(model) = handle.as_any().downcast_ref::<VecModel<WorkspaceTab>>() {
                for (index, next) in tabs.iter().enumerate() {
                    let Some(row) = model.row_data(index) else {
                        continue;
                    };
                    if !workspace_tab_matches(&row, next) {
                        model.set_row_data(index, workspace_tab_from(next));
                    }
                }
                return;
            }
        }
    }
    window.set_tabs(ModelRc::new(VecModel::from(
        tabs.iter().map(workspace_tab_from).collect::<Vec<_>>(),
    )));
}

// --- N2：Quick Connect 行模型（与 SFTP/标签条同样的"变了才重建"策略）----------

fn quick_connect_row_from(row: &crate::runtime::QuickConnectRowData) -> QuickConnectRow {
    QuickConnectRow {
        kind: row.kind.clone().into(),
        title: row.title.clone().into(),
        target: row.target.clone().into(),
        last_used_text: row.last_used_text.clone().into(),
    }
}

fn quick_connect_row_matches(
    current: &QuickConnectRow,
    next: &crate::runtime::QuickConnectRowData,
) -> bool {
    current.kind.as_str() == next.kind
        && current.title.as_str() == next.title
        && current.target.as_str() == next.target
        && current.last_used_text.as_str() == next.last_used_text
}

fn set_quick_connect_rows_if_changed(
    window: &MainWindow,
    rows: &[crate::runtime::QuickConnectRowData],
) {
    let current = window.get_quick_connect_rows();
    if current.row_count() == rows.len() {
        let unchanged = (0..rows.len()).all(|index| {
            current
                .row_data(index)
                .is_some_and(|row| quick_connect_row_matches(&row, &rows[index]))
        });
        if unchanged {
            return;
        }
    }
    window.set_quick_connect_rows(ModelRc::new(VecModel::from(
        rows.iter().map(quick_connect_row_from).collect::<Vec<_>>(),
    )));
}

fn quick_link_row_from(row: &crate::runtime::QuickLinkRowData) -> QuickLinkRow {
    QuickLinkRow {
        id: row.id.clone().into(),
        label: row.label.clone().into(),
        target: row.target.clone().into(),
    }
}

fn quick_link_row_matches(current: &QuickLinkRow, next: &crate::runtime::QuickLinkRowData) -> bool {
    current.id.as_str() == next.id
        && current.label.as_str() == next.label
        && current.target.as_str() == next.target
}

fn set_quick_links_rows_if_changed(window: &MainWindow, rows: &[crate::runtime::QuickLinkRowData]) {
    let current = window.get_quick_links_rows();
    if current.row_count() == rows.len() {
        let unchanged = (0..rows.len()).all(|index| {
            current
                .row_data(index)
                .is_some_and(|row| quick_link_row_matches(&row, &rows[index]))
        });
        if unchanged {
            return;
        }
    }
    window.set_quick_links_rows(ModelRc::new(VecModel::from(
        rows.iter().map(quick_link_row_from).collect::<Vec<_>>(),
    )));
}

// --- N4：密钥管理页 / 认证弹窗的模型转换（带变更检测，避免重建行实例）--------

fn private_key_row_from(row: &PrivateKeyRowData) -> PrivateKeyRow {
    PrivateKeyRow {
        id: row.id.clone().into(),
        label: row.label.clone().into(),
        algorithm: row.algorithm.clone().into(),
        fingerprint: row.fingerprint.clone().into(),
        passphrase_stored: row.passphrase_stored,
        material_present: row.material_present,
        used_by: row.used_by.clone().into(),
        selected: row.selected,
    }
}

fn private_key_row_matches(current: &PrivateKeyRow, next: &PrivateKeyRowData) -> bool {
    current.id == next.id.as_str()
        && current.label == next.label.as_str()
        && current.algorithm == next.algorithm.as_str()
        && current.fingerprint == next.fingerprint.as_str()
        && current.passphrase_stored == next.passphrase_stored
        && current.material_present == next.material_present
        && current.used_by == next.used_by.as_str()
        && current.selected == next.selected
}

fn set_private_keys_rows_if_changed(window: &MainWindow, rows: &[PrivateKeyRowData]) {
    let current = window.get_private_keys_rows();
    if current.row_count() == rows.len() {
        let unchanged = (0..rows.len()).all(|index| {
            current
                .row_data(index)
                .is_some_and(|row| private_key_row_matches(&row, &rows[index]))
        });
        if unchanged {
            return;
        }
    }
    window.set_private_keys_rows(ModelRc::new(VecModel::from(
        rows.iter().map(private_key_row_from).collect::<Vec<_>>(),
    )));
}

fn host_key_entry_from(entry: &crate::runtime::HostKeyEntryData) -> HostKeyEntry {
    HostKeyEntry {
        algorithm: entry.algorithm.clone().into(),
        fingerprint: entry.fingerprint.clone().into(),
        key_line: entry.key_line.clone().into(),
    }
}

fn host_key_group_from(group: &HostKeyGroupData) -> HostKeyGroup {
    HostKeyGroup {
        host: group.host.clone().into(),
        port: i32::from(group.port),
        entries: ModelRc::new(VecModel::from(
            group
                .entries
                .iter()
                .map(host_key_entry_from)
                .collect::<Vec<_>>(),
        )),
    }
}

fn host_key_group_matches(current: &HostKeyGroup, next: &HostKeyGroupData) -> bool {
    if current.host != next.host.as_str() || current.port != i32::from(next.port) {
        return false;
    }
    let current_entries = current.entries.clone();
    current_entries.row_count() == next.entries.len()
        && (0..next.entries.len()).all(|index| {
            current_entries.row_data(index).is_some_and(|entry| {
                entry.algorithm == next.entries[index].algorithm.as_str()
                    && entry.fingerprint == next.entries[index].fingerprint.as_str()
                    && entry.key_line == next.entries[index].key_line.as_str()
            })
        })
}

fn set_host_keys_groups_if_changed(window: &MainWindow, groups: &[HostKeyGroupData]) {
    let current = window.get_host_keys_groups();
    if current.row_count() == groups.len() {
        let unchanged = (0..groups.len()).all(|index| {
            current
                .row_data(index)
                .is_some_and(|group| host_key_group_matches(&group, &groups[index]))
        });
        if unchanged {
            return;
        }
    }
    window.set_host_keys_groups(ModelRc::new(VecModel::from(
        groups.iter().map(host_key_group_from).collect::<Vec<_>>(),
    )));
}

fn auth_key_option_from(option: &AuthKeyOptionData) -> AuthKeyOption {
    AuthKeyOption {
        id: option.id.clone().into(),
        label: option.label.clone().into(),
        detail: option.detail.clone().into(),
    }
}

fn set_auth_key_options_if_changed(window: &MainWindow, options: &[AuthKeyOptionData]) {
    let current = window.get_auth_prompt_key_options();
    if current.row_count() == options.len() {
        let unchanged = (0..options.len()).all(|index| {
            current.row_data(index).is_some_and(|option| {
                option.id == options[index].id.as_str()
                    && option.label == options[index].label.as_str()
                    && option.detail == options[index].detail.as_str()
            })
        });
        if unchanged {
            return;
        }
    }
    window.set_auth_prompt_key_options(ModelRc::new(VecModel::from(
        options.iter().map(auth_key_option_from).collect::<Vec<_>>(),
    )));
}

fn auth_keyboard_prompt_from(prompt: &AuthPromptQuestionData) -> AuthKeyboardPrompt {
    AuthKeyboardPrompt {
        text: prompt.text.clone().into(),
        echo: prompt.echo,
        answer: prompt.answer.clone().into(),
    }
}

fn set_auth_challenge_prompts_if_changed(window: &MainWindow, prompts: &[AuthPromptQuestionData]) {
    let current = window.get_auth_prompt_challenge_prompts();
    if current.row_count() == prompts.len() {
        let unchanged = (0..prompts.len()).all(|index| {
            current.row_data(index).is_some_and(|prompt| {
                prompt.text == prompts[index].text.as_str()
                    && prompt.echo == prompts[index].echo
                    && prompt.answer == prompts[index].answer.as_str()
            })
        });
        if unchanged {
            return;
        }
    }
    window.set_auth_prompt_challenge_prompts(ModelRc::new(VecModel::from(
        prompts
            .iter()
            .map(auth_keyboard_prompt_from)
            .collect::<Vec<_>>(),
    )));
}

fn session_tree_row_from(row: &crate::runtime::SessionTreeRow) -> SessionTreeRow {
    SessionTreeRow {
        kind: row.kind.clone().into(),
        depth: row.depth,
        label: row.label.clone().into(),
        detail: row.detail.clone().into(),
        id: row.id.clone().into(),
        expanded: row.expanded,
        selected: row.selected,
    }
}

/// Compares one projected row with the row currently in the Slint model.
///
/// `selected` is deliberately **not** compared: the row highlight is driven by
/// the scalar `saved_session_tree_selected_id` property, so a pure selection
/// change must not swap the model. Swapping it recreates every row instance and
/// Slint resets its click counter whenever the item under the cursor changes
/// (`i-slint-core` `window.rs`: `click_state.reset()`), which turns a
/// double-click into two single clicks — i.e. "double-click a session to open
/// it" stops working.
fn session_tree_row_matches(
    current: &SessionTreeRow,
    next: &crate::runtime::SessionTreeRow,
) -> bool {
    current.kind.as_str() == next.kind
        && current.depth == next.depth
        && current.label.as_str() == next.label
        && current.detail.as_str() == next.detail
        && current.id.as_str() == next.id
        && current.expanded == next.expanded
}

/// Replaces the session-tree row model only when the projection changed, so a
/// selection/expansion update does not recreate every row instance (which would
/// reset the tree's Flickable scroll position).
fn set_session_tree_rows_if_changed(window: &MainWindow, rows: &[crate::runtime::SessionTreeRow]) {
    let current = window.get_session_tree_rows();
    let unchanged = current.row_count() == rows.len()
        && current
            .iter()
            .zip(rows)
            .all(|(current, next)| session_tree_row_matches(&current, next));
    if unchanged {
        return;
    }
    window.set_session_tree_rows(ModelRc::new(VecModel::from(
        rows.iter().map(session_tree_row_from).collect::<Vec<_>>(),
    )));
}

/// 会话树点击/激活的输入状态（每个窗口一份）。
///
/// 为什么需要它：Slint 的双击判定窗口固定 500ms，并且"光标下的 item 变化"会重置
/// 计数（`i-slint-core` `window.rs` 的 `click_state.reset()`）。本应用在软件渲染 +
/// WSLg 下，一次点击引起的重绘会让第二次点击晚到 ~600ms（实测 613–667ms），原生
/// `double-clicked` 因此不触发。这里在 `select` 路径上补一个更宽松的
/// "重复点击 = 激活"兜底，两条路径共用一个去重窗口，保证一次双击只激活一次。
#[derive(Default)]
struct SessionTreeInput {
    last_click: Option<(String, std::time::Instant)>,
    last_activation: Option<(String, std::time::Instant)>,
}

/// 重复点击窗口：要覆盖环境重绘延迟，同时仍在"慢速双击"的合理范围内。
///
/// 实测（debug + 软件渲染 + Xvfb，见 `.tmp-tree-click-check.sh` 的
/// `[tree-click-trace]` 轨迹）：
/// * 无活动会话时，第二次点击的回调距第一次 ~465ms（重绘较快）；
/// * 一旦有会话在渲染终端画面，同一路径的重绘延迟涨到 ~1.24s，
///   第二次点击的回调落在旧的 900ms 窗口之外，双击退化成两次单击（只选中）。
///
/// 窗口放宽到 2.5s：覆盖有会话时的重绘延迟（实测 1.24s，留 2 倍余量），
/// 同时仍明显短于用户两次独立点击同一行的常见间隔（轨迹里为 3.7–4.2s），
/// 不会把两次单击误判成双击。
const SESSION_TREE_REPEAT_CLICK_WINDOW: Duration = Duration::from_millis(2500);
/// 去重窗口：`double-clicked` 与重复点击兜底会在同一帧内先后命中同一个节点。
const SESSION_TREE_ACTIVATION_DEDUP_WINDOW: Duration = Duration::from_millis(300);

/// 记录一次节点点击，返回它是否落在重复点击窗口内（= 双击的第二下）。
fn session_tree_register_click(state: &Rc<RefCell<SessionTreeInput>>, id: &str) -> bool {
    let now = std::time::Instant::now();
    let mut state = state.borrow_mut();
    let repeated = state.last_click.as_ref().is_some_and(|(last_id, at)| {
        last_id == id && now.duration_since(*at) <= SESSION_TREE_REPEAT_CLICK_WINDOW
    });
    state.last_click = Some((id.to_owned(), now));
    repeated
}

/// 认领一次激活；同一节点在去重窗口内的重复认领返回 `false`。
fn session_tree_claim_activation(state: &Rc<RefCell<SessionTreeInput>>, id: &str) -> bool {
    let now = std::time::Instant::now();
    let mut state = state.borrow_mut();
    let duplicate = state.last_activation.as_ref().is_some_and(|(last_id, at)| {
        last_id == id && now.duration_since(*at) <= SESSION_TREE_ACTIVATION_DEDUP_WINDOW
    });
    if duplicate {
        return false;
    }
    state.last_activation = Some((id.to_owned(), now));
    true
}

/// 激活一个会话树节点；被去重窗口拦下时返回 `None`（调用方直接跳过）。
fn session_tree_activate(
    state: &Rc<RefCell<SessionTreeInput>>,
    runtime: &Rc<RefCell<AppRuntime>>,
    id: &str,
) -> Option<AppResult<AppProjection>> {
    if !session_tree_claim_activation(state, id) {
        return None;
    }
    Some(runtime.borrow_mut().activate_saved_session_tree_node(id))
}

/// Sets a plain (not-yet-migrated) status line and clears the i18n kind, so the
/// status bar falls back to the English `status_text` after error paths that
/// bypass [`apply_projection`] (which always re-applies kind + params).
fn set_plain_status(window: &MainWindow, text: SharedString) {
    window.set_status_text(text);
    window.set_status_kind_text("".into());
    window.set_status_param_1_text("".into());
    window.set_status_param_2_text("".into());
}

/// N3：面板 id 字符串 → `PanelId`（Slint 回调参数）。
fn panel_id_from_str(id: &str) -> Option<PanelId> {
    match id {
        "sessions" => Some(PanelId::Sessions),
        "sftp" => Some(PanelId::Sftp),
        "tunnels" => Some(PanelId::Tunnels),
        "quick_commands" => Some(PanelId::QuickCommands),
        "transfers" => Some(PanelId::Transfers),
        _ => None,
    }
}

/// N3：侧 id 字符串 → `PanelSide`。
fn panel_side_from_str(side: &str) -> Option<PanelSide> {
    match side {
        "left" => Some(PanelSide::Left),
        "right" => Some(PanelSide::Right),
        _ => None,
    }
}

/// N3：Rust 面板框投影 → Slint 同构结构体。
fn panel_frame_data(frame: &RuntimePanelFrameData) -> PanelFrameData {
    PanelFrameData {
        placement: frame.placement.clone().into(),
        collapsed: frame.collapsed,
        visible: frame.visible,
        x: frame.x as f32,
        y: frame.y as f32,
        width: frame.width as f32,
        height: frame.height as f32,
    }
}

/// N3：Rust 分栏手柄投影 → Slint 同构结构体。
fn split_handle_data(handle: &RuntimeSplitHandleData) -> SplitHandleData {
    SplitHandleData {
        side: handle.side.clone().into(),
        boundary: handle.boundary,
        x: handle.x as f32,
        y: handle.y as f32,
        width: handle.width as f32,
        height: handle.height as f32,
    }
}

fn apply_projection(window: &MainWindow, projection: &AppProjection) {
    window.set_config_dir(projection.config_dir_text.clone().into());
    window.set_secret_store_kind_text(projection.secret_store_kind_text.clone().into());
    window.set_secret_store_path_text(projection.secret_store_path_text.clone().into());
    window.set_secret_reset_confirmation_text(
        projection.secret_reset_confirmation_text.clone().into(),
    );
    window.set_editor_folder_label_text(projection.editor_folder_label_text.clone().into());
    window.set_editor_folder_id_text(projection.editor_folder_id_text.clone().into());
    window.set_editor_folder_known(projection.editor_folder_known);
    window.set_new_folder_name_text(projection.new_folder_name_text.clone().into());
    window.set_editor_name_text(projection.editor_name_text.clone().into());
    window.set_editor_host_text(projection.editor_host_text.clone().into());
    window.set_editor_port_text(projection.editor_port_text.clone().into());
    window.set_editor_username_text(projection.editor_username_text.clone().into());
    window.set_editor_auth_method_text(projection.editor_auth_method_text.clone().into());
    window.set_editor_host_key_policy_text(projection.editor_host_key_policy_text.clone().into());
    window.set_editor_password_text(projection.editor_password_text.clone().into());
    window.set_editor_key_path_text(projection.editor_key_path_text.clone().into());
    window.set_editor_passphrase_text(projection.editor_passphrase_text.clone().into());
    window
        .set_editor_target_session_id_text(projection.editor_target_session_id_text.clone().into());
    window.set_editor_target_editing(projection.editor_target_editing);
    window.set_editor_modal_visible(projection.editor_modal_visible);
    window.set_editor_section_text(projection.editor_section_text.clone().into());
    window.set_editor_auth_test_kind_text(projection.editor_auth_test_kind_text.clone().into());
    window.set_editor_auth_test_host_text(projection.editor_auth_test_host_text.clone().into());
    window
        .set_editor_auth_test_backend_text(projection.editor_auth_test_backend_text.clone().into());
    window
        .set_editor_auth_test_startup_text(projection.editor_auth_test_startup_text.clone().into());
    window.set_editor_auth_test_error_text(projection.editor_auth_test_error_text.clone().into());
    window.set_known_hosts_modal_visible(projection.known_hosts_modal_visible);
    window.set_known_hosts_inventory_rows_text(
        projection.known_hosts_inventory_rows_text.clone().into(),
    );
    window.set_known_hosts_inventory_empty(projection.known_hosts_inventory_empty);
    window.set_known_hosts_selection_kind_text(
        projection.known_hosts_selection_kind_text.clone().into(),
    );
    window.set_known_hosts_selection_host_text(
        projection.known_hosts_selection_host_text.clone().into(),
    );
    window.set_known_hosts_selection_port_text(
        projection.known_hosts_selection_port_text.clone().into(),
    );
    window.set_known_hosts_selection_index(projection.known_hosts_selection_index);
    window.set_known_hosts_selection_total(projection.known_hosts_selection_total);
    window.set_known_hosts_details_text(projection.known_hosts_details_text.clone().into());
    window.set_known_hosts_path_text(projection.known_hosts_path_text.clone().into());
    window.set_known_hosts_clear_confirmation_text(
        projection
            .known_hosts_clear_confirmation_text
            .clone()
            .into(),
    );
    window.set_editor_proxy_mode_text(projection.editor_proxy_mode_text.clone().into());
    window.set_editor_proxy_protocol_text(projection.editor_proxy_protocol_text.clone().into());
    window.set_editor_proxy_host_text(projection.editor_proxy_host_text.clone().into());
    window.set_editor_proxy_port_text(projection.editor_proxy_port_text.clone().into());
    window.set_editor_proxy_username_text(projection.editor_proxy_username_text.clone().into());
    window.set_editor_proxy_password_text(projection.editor_proxy_password_text.clone().into());
    window.set_editor_proxy_dns_by_proxy(projection.editor_proxy_dns_by_proxy);
    window.set_editor_proxy_summary_kind_text(
        projection.editor_proxy_summary_kind_text.clone().into(),
    );
    window.set_editor_proxy_summary_address_text(
        projection.editor_proxy_summary_address_text.clone().into(),
    );
    window.set_editor_proxy_summary_user_text(
        projection.editor_proxy_summary_user_text.clone().into(),
    );
    window.set_editor_tunnel_kind_text(projection.editor_tunnel_kind_text.clone().into());
    window.set_editor_tunnel_bind_host_text(projection.editor_tunnel_bind_host_text.clone().into());
    window.set_editor_tunnel_bind_port_text(projection.editor_tunnel_bind_port_text.clone().into());
    window.set_editor_tunnel_target_host_text(
        projection.editor_tunnel_target_host_text.clone().into(),
    );
    window.set_editor_tunnel_target_port_text(
        projection.editor_tunnel_target_port_text.clone().into(),
    );
    window.set_editor_tunnel_summary_kind_text(
        projection.editor_tunnel_summary_kind_text.clone().into(),
    );
    window.set_editor_tunnel_summary_rows_text(
        projection.editor_tunnel_summary_rows_text.clone().into(),
    );
    window.set_editor_tunnel_summary_count(projection.editor_tunnel_summary_count);
    window.set_logging_enabled_text(projection.logging_enabled_text.clone().into());
    window.set_logging_format_text(projection.logging_format_text.clone().into());
    window.set_logging_directory_text(projection.logging_directory_text.clone().into());
    window.set_logging_directory_display_text(
        projection.logging_directory_display_text.clone().into(),
    );
    window.set_settings_scrollback_lines_text(
        projection.settings_scrollback_lines_text.clone().into(),
    );
    window.set_settings_scrollback_max_cells_text(
        projection.settings_scrollback_max_cells_text.clone().into(),
    );
    window.set_settings_terminal_status_kind_text(
        projection.settings_terminal_status_kind_text.clone().into(),
    );
    window.set_settings_terminal_status_field_text(
        projection
            .settings_terminal_status_field_text
            .clone()
            .into(),
    );
    window.set_settings_terminal_status_value_text(
        projection
            .settings_terminal_status_value_text
            .clone()
            .into(),
    );
    window.set_settings_terminal_status_limit_text(
        projection
            .settings_terminal_status_limit_text
            .clone()
            .into(),
    );
    window.set_active_session_kind_text(projection.active_session_kind_text.clone().into());
    window.set_active_session_name_text(projection.active_session_name_text.clone().into());
    window.set_active_session_state_text(projection.active_session_state_text.clone().into());
    // W5-A3：菜单/按钮状态感知所需的布尔量。
    window.set_has_active_session(projection.has_active_session);
    window.set_active_session_connected(projection.active_session_connected);
    window.set_has_saved_selection(projection.has_saved_selection);
    window.set_saved_session_count(projection.saved_session_count);
    window.set_session_summary_rows_text(projection.session_summary_rows_text.clone().into());
    window.set_session_summary_count(projection.session_summary_count);
    window.set_session_summary_hidden_count(projection.session_summary_hidden_count);
    window.set_session_search_text(projection.session_search_text.clone().into());
    window.set_saved_session_selection_kind_text(
        projection.saved_session_selection_kind_text.clone().into(),
    );
    window.set_saved_session_selection_name_text(
        projection.saved_session_selection_name_text.clone().into(),
    );
    window.set_saved_session_selection_host_text(
        projection.saved_session_selection_host_text.clone().into(),
    );
    window.set_saved_session_selection_id_text(
        projection.saved_session_selection_id_text.clone().into(),
    );
    window.set_saved_session_inventory_rows_text(
        projection.saved_session_inventory_rows_text.clone().into(),
    );
    window.set_saved_session_inventory_query_text(
        projection.saved_session_inventory_query_text.clone().into(),
    );
    window.set_saved_session_inventory_empty(projection.saved_session_inventory_empty);
    set_session_tree_rows_if_changed(window, &projection.session_tree_rows);
    window.set_saved_session_tree_selected_id(
        projection
            .session_tree_rows
            .iter()
            .find(|row| row.selected)
            .map(|row| row.id.clone())
            .unwrap_or_default()
            .into(),
    );
    window.set_recent_sessions_rows_text(projection.recent_sessions_rows_text.clone().into());
    window.set_recent_sessions_empty(projection.recent_sessions_empty);
    window.set_host_key_prompt_visible(projection.host_key_prompt_visible);
    window.set_host_key_prompt_text(projection.host_key_prompt_text.clone().into());
    window.set_host_key_prompt_mode_text(projection.host_key_prompt_mode_text.clone().into());
    window.set_host_key_prompt_confirmation_text(
        projection.host_key_prompt_confirmation_text.clone().into(),
    );
    // W5：密码弹窗（仅本次连接使用）；关闭后清空输入，避免残留上一次的密码。
    window.set_password_prompt_visible(projection.password_prompt_visible);
    window.set_password_prompt_host_text(projection.password_prompt_host_text.clone().into());
    if !projection.password_prompt_visible && !window.get_password_prompt_value_text().is_empty() {
        window.set_password_prompt_value_text("".into());
    }
    // N6：终端日志（菜单/状态栏入口、REC 指示、弹窗字段）。
    window.set_logging_active(projection.logging_active);
    window.set_logging_mode_text(projection.logging_mode_text.clone().into());
    window.set_logging_path_text(projection.logging_path_text.clone().into());
    window.set_logging_file_name_text(projection.logging_file_name_text.clone().into());
    window.set_terminal_logging_start_enabled(projection.terminal_logging_start_enabled);
    window.set_terminal_logging_stop_enabled(projection.terminal_logging_stop_enabled);
    window.set_terminal_logging_open_file_enabled(projection.terminal_logging_open_file_enabled);
    window
        .set_terminal_logging_open_folder_enabled(projection.terminal_logging_open_folder_enabled);
    window.set_logging_dialog_visible(projection.logging_dialog_visible);
    window.set_logging_dialog_session_text(projection.logging_dialog_session_text.clone().into());
    // 文本字段与输入框双向绑定：值相同不重设，避免打断正在编辑的光标/选区。
    if window.get_logging_dialog_directory_text().as_str()
        != projection.logging_dialog_directory_text
    {
        window.set_logging_dialog_directory_text(
            projection.logging_dialog_directory_text.clone().into(),
        );
    }
    if window.get_logging_dialog_file_name_text().as_str()
        != projection.logging_dialog_file_name_text
    {
        window.set_logging_dialog_file_name_text(
            projection.logging_dialog_file_name_text.clone().into(),
        );
    }
    window.set_logging_dialog_file_name_error_text(
        projection
            .logging_dialog_file_name_error_text
            .clone()
            .into(),
    );
    window.set_logging_dialog_path_error_text(
        projection.logging_dialog_path_error_text.clone().into(),
    );
    window.set_logging_dialog_file_exists(projection.logging_dialog_file_exists);
    window.set_logging_dialog_raw_format(projection.logging_dialog_raw_format);
    window.set_logging_dialog_sanitized_format(!projection.logging_dialog_raw_format);
    window.set_logging_dialog_timestamps(projection.logging_dialog_timestamps);
    window.set_logging_dialog_include_input(projection.logging_dialog_include_input);
    window.set_logging_dialog_input_confirmed(projection.logging_dialog_input_confirmed);
    window.set_logging_dialog_overwrite_confirmed(projection.logging_dialog_overwrite_confirmed);
    window.set_tab_name_text(projection.tab_name_text.clone().into());
    window.set_tab_state_text(projection.tab_state_text.clone().into());
    window.set_tab_has_session(projection.tab_has_session);
    // N0：标签条模型 + 关闭确认/右键菜单状态。
    set_tabs_if_changed(window, &projection.tabs);
    window.set_tab_count(projection.tab_count);
    window.set_tab_has_disconnected(projection.tab_has_disconnected);
    // N2：Quick Connect 页投影（可见性/输入/错误/摘要/最近连接/快速链接）。
    window.set_quick_connect_visible(projection.quick_connect_visible);
    if window.get_quick_connect_input_text().as_str() != projection.quick_connect_input_text {
        window.set_quick_connect_input_text(projection.quick_connect_input_text.clone().into());
    }
    window.set_quick_connect_error_text(projection.quick_connect_error_text.clone().into());
    window.set_quick_connect_summary_text(projection.quick_connect_summary_text.clone().into());
    window.set_quick_connect_last_target_text(
        projection.quick_connect_last_target_text.clone().into(),
    );
    window.set_quick_connect_history_enabled(projection.quick_connect_history_enabled);
    set_quick_connect_rows_if_changed(window, &projection.quick_connect_rows);
    set_quick_links_rows_if_changed(window, &projection.quick_links_rows);
    window.set_close_tabs_confirm_visible(projection.close_tabs_confirm_visible);
    window.set_close_tabs_confirm_single(projection.close_tabs_confirm_single);
    window.set_close_tabs_confirm_name_text(projection.close_tabs_confirm_name_text.clone().into());
    window.set_close_tabs_confirm_count(projection.close_tabs_confirm_count);
    window.set_close_tabs_confirm_active_count(projection.close_tabs_confirm_active_count);
    window.set_tab_menu_close_others_enabled(projection.tab_menu_close_others_enabled);
    window.set_tab_menu_close_left_enabled(projection.tab_menu_close_left_enabled);
    window.set_tab_menu_close_right_enabled(projection.tab_menu_close_right_enabled);
    window.set_tab_menu_close_all_enabled(projection.tab_menu_close_all_enabled);
    window.set_tab_menu_close_disconnected_enabled(projection.tab_menu_close_disconnected_enabled);
    // W5-A2 的终端位图缓存按"活动标签"判断是否需要重绘：标签 id 变化必须使
    // 缓存失效（同 frame_id 的不同会话切回来也要重绘）。
    window.set_active_session(projection.active_tab_id.clone().into());
    window.set_terminal_title_name_text(projection.terminal_title_name_text.clone().into());
    window.set_terminal_title_has_session(projection.terminal_title_has_session);
    window.set_terminal_body_text(projection.terminal_body_text.clone().into());
    window.set_terminal_body_kind_text(projection.terminal_body_kind_text.clone().into());
    window.set_terminal_visible_lines(ModelRc::new(VecModel::from(
        projection
            .terminal_visible_lines
            .iter()
            .cloned()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    window.set_terminal_cursor_column(projection.terminal_cursor_column);
    window.set_terminal_cursor_row(projection.terminal_cursor_row);
    window.set_terminal_scroll_offset(
        i32::try_from(projection.terminal_scroll_offset).unwrap_or(i32::MAX),
    );
    // 滚动条几何（§5.17）：终端视图默认从 TerminalScrollInfo 读取总行数与视口行数。
    {
        let scrollbar = window.global::<TerminalScrollInfo>();
        scrollbar.set_scrollback_lines(projection.terminal_scrollback_lines);
        scrollbar.set_viewport_rows(projection.terminal_viewport_rows);
    }
    window.set_terminal_selection_active(projection.terminal_selection_active);
    window.set_terminal_has_selection(projection.terminal_has_selection);
    window.set_terminal_search_query_text(projection.terminal_search_query_text.clone().into());
    window.set_terminal_search_kind_text(projection.terminal_search_kind_text.clone().into());
    window.set_terminal_search_match_count(projection.terminal_search_match_count);
    window.set_terminal_search_current_index(projection.terminal_search_current_index);
    window.set_sftp_path_text(projection.sftp_path_text.clone().into());
    window.set_sftp_listing_kind_text(projection.sftp_listing_kind_text.clone().into());
    window.set_sftp_listing_rows_text(projection.sftp_listing_rows_text.clone().into());
    window
        .set_sftp_listing_session_key_text(projection.sftp_listing_session_key_text.clone().into());
    window.set_sftp_listing_path_text(projection.sftp_listing_path_text.clone().into());
    window.set_sftp_listing_error_text(projection.sftp_listing_error_text.clone().into());
    window.set_sftp_local_path_text(projection.sftp_local_path_text.clone().into());
    window.set_sftp_remote_target_text(projection.sftp_remote_target_text.clone().into());
    window.set_sftp_secondary_target_text(projection.sftp_secondary_target_text.clone().into());
    window.set_sftp_permissions_text(projection.sftp_permissions_text.clone().into());
    window
        .set_sftp_session_status_kind_text(projection.sftp_session_status_kind_text.clone().into());
    window.set_sftp_session_status_reason_text(
        projection.sftp_session_status_reason_text.clone().into(),
    );
    window.set_sftp_session_status_detail_text(
        projection.sftp_session_status_detail_text.clone().into(),
    );
    window.set_sftp_session_status_session_key_text(
        projection
            .sftp_session_status_session_key_text
            .clone()
            .into(),
    );
    window.set_sftp_remote_edit_remote_path_text(
        projection.sftp_remote_edit_remote_path_text.clone().into(),
    );
    window.set_sftp_remote_edit_local_path_text(
        projection.sftp_remote_edit_local_path_text.clone().into(),
    );
    set_sftp_rows_if_changed(window, &projection.sftp_rows);
    set_sftp_crumbs_if_changed(window, &projection.sftp_crumbs);
    window.set_sftp_selected_index(projection.sftp_selected_index);
    window.set_sftp_sort_column_text(projection.sftp_sort_column_text.clone().into());
    window.set_sftp_sort_ascending(projection.sftp_sort_ascending);
    window.set_sftp_show_hidden(projection.sftp_show_hidden);
    window.set_sftp_item_summary_text(projection.sftp_item_summary_text.clone().into());
    window.set_sftp_item_count(projection.sftp_item_count);
    window.set_sftp_dir_count(projection.sftp_dir_count);
    window.set_sftp_empty_text(projection.sftp_empty_text.clone().into());
    window.set_sftp_selected_name_text(projection.sftp_selected_name_text.clone().into());
    window.set_sftp_selected_path_text(projection.sftp_selected_path_text.clone().into());
    window.set_sftp_selected_permissions_text(
        projection.sftp_selected_permissions_text.clone().into(),
    );
    window.set_sftp_remote_edit_active(projection.sftp_remote_edit_active);
    window.set_sftp_available(projection.sftp_available);
    window.set_transfer_queue_rows_text(projection.transfer_queue_rows_text.clone().into());
    window.set_transfer_queue_empty(projection.transfer_queue_empty);
    // N1 Phase 2：本地栏 / 队列抽屉 / 冲突 / 属性 / 剪贴板。
    set_local_rows_if_changed(window, &projection.local_rows);
    window.set_local_path_text(projection.local_path_text.clone().into());
    window.set_local_sort_column_text(projection.local_sort_column_text.clone().into());
    window.set_local_sort_ascending(projection.local_sort_ascending);
    window.set_local_show_hidden(projection.local_show_hidden);
    window.set_local_item_count(projection.local_item_count);
    window.set_local_dir_count(projection.local_dir_count);
    window.set_local_empty_text(projection.local_empty_text.clone().into());
    window.set_local_error_text(projection.local_error_text.clone().into());
    window.set_local_retry_visible(projection.local_error_retryable);
    window.set_local_selected_count(projection.local_selected_count);
    window.set_local_collapsed(projection.local_collapsed);
    window.set_local_selected_name_text(projection.local_selected_name_text.clone().into());
    window.set_local_selection_key_text(projection.local_selection_key_text.clone().into());
    window.set_sftp_selection_key_text(projection.sftp_selection_key_text.clone().into());
    set_transfer_rows_if_changed(window, &projection.transfer_rows);
    window.set_transfer_drawer_expanded(projection.transfer_drawer_expanded);
    window.set_transfer_total_count(projection.transfer_total_count);
    window.set_transfer_active_count(projection.transfer_active_count);
    window.set_transfer_paused_count(projection.transfer_paused_count);
    window.set_transfer_failed_count(projection.transfer_failed_count);
    window.set_transfer_completed_count(projection.transfer_completed_count);
    window.set_transfer_retryable_count(projection.transfer_retryable_count);
    window.set_sftp_conflict_visible(projection.sftp_conflict_visible);
    window.set_sftp_conflict_source_text(projection.sftp_conflict_source_text.clone().into());
    window.set_sftp_conflict_destination_text(
        projection.sftp_conflict_destination_text.clone().into(),
    );
    window.set_sftp_conflict_count(projection.sftp_conflict_count);
    window.set_sftp_conflict_paths_text(projection.sftp_conflict_paths_text.clone().into());
    window.set_sftp_properties_visible(projection.sftp_properties_visible);
    window.set_sftp_properties_name_text(projection.sftp_properties_name_text.clone().into());
    window.set_sftp_properties_path_text(projection.sftp_properties_path_text.clone().into());
    window.set_sftp_properties_kind_text(projection.sftp_properties_kind_text.clone().into());
    window.set_sftp_properties_size_text(projection.sftp_properties_size_text.clone().into());
    window
        .set_sftp_properties_modified_text(projection.sftp_properties_modified_text.clone().into());
    window.set_sftp_properties_permissions_text(
        projection.sftp_properties_permissions_text.clone().into(),
    );
    window.set_clipboard_side_text(projection.clipboard_side_text.clone().into());
    window.set_clipboard_count(projection.clipboard_count);
    window.set_clipboard_cut(projection.clipboard_cut);
    window.set_sftp_menu_target_kind_text(projection.sftp_menu_target_kind_text.clone().into());
    window.set_sftp_selected_count(projection.sftp_selected_count);
    window.set_sftp_selection_single_file(projection.sftp_selection_single_file);
    window.set_tunnels_summary_kind_text(projection.tunnels_summary_kind_text.clone().into());
    window.set_tunnels_summary_rows_text(projection.tunnels_summary_rows_text.clone().into());
    window.set_status_text(projection.status_text.clone().into());
    window.set_status_kind_text(projection.status_kind.clone().into());
    window.set_status_param_1_text(projection.status_param_1.clone().into());
    window.set_status_param_2_text(projection.status_param_2.clone().into());
    window.set_sftp_visible(projection.sftp_visible);
    window.set_tunnels_visible(projection.tunnels_visible);
    window.set_commands_visible(projection.commands_visible);
    // --- N3：面板停靠布局（Rust px 几何 → Slint 普通属性，无 width 反推）------
    window.set_sessions_visible(projection.sessions_visible);
    window.set_transfers_visible(projection.transfers_visible);
    window.set_layout_left_effective_width(projection.layout.left_effective_width_px as f32);
    window.set_layout_right_effective_width(projection.layout.right_effective_width_px as f32);
    window.set_layout_left_width(projection.layout.left_width_px as f32);
    window.set_layout_right_width(projection.layout.right_width_px as f32);
    window.set_layout_rail_active(projection.layout.nav_rail_active);
    window.set_layout_rail_available(projection.layout.nav_rail_available);
    window.set_layout_dock_toggle_visible(projection.layout.dock_toggle_visible);
    window.set_layout_dock_collapsed(projection.layout.dock_auto_collapsed);
    window.set_layout_drag_scale(projection.layout.layout_drag_scale_permille as f32 / 1000.0);
    window.set_layout_sessions_frame(panel_frame_data(&projection.layout.sessions_frame));
    window.set_layout_sftp_frame(panel_frame_data(&projection.layout.sftp_frame));
    window.set_layout_tunnels_frame(panel_frame_data(&projection.layout.tunnels_frame));
    window
        .set_layout_quick_commands_frame(panel_frame_data(&projection.layout.quick_commands_frame));
    window.set_layout_transfers_frame(panel_frame_data(&projection.layout.transfers_frame));
    let handles = projection
        .layout
        .split_handles
        .iter()
        .map(split_handle_data)
        .collect::<Vec<_>>();
    window.set_layout_split_handles(ModelRc::new(VecModel::from(handles)));
    window.set_layout_drag_indicator_visible(projection.layout.drag_indicator.is_some());
    if let Some(indicator) = &projection.layout.drag_indicator {
        window.set_layout_drag_indicator_x(indicator.x as f32);
        window.set_layout_drag_indicator_y(indicator.y as f32);
        window.set_layout_drag_indicator_width(indicator.width as f32);
    }
    window.set_app_version_text(projection.app_version_text.clone().into());
    // N4：私钥管理页（文本字段只在差异时写回，避免打断输入）。
    window.set_private_keys_modal_visible(projection.private_keys_modal_visible);
    set_private_keys_rows_if_changed(window, &projection.private_keys_rows);
    window.set_private_keys_selected_index(projection.private_keys_selected_index);
    window.set_private_keys_summary_text(projection.private_keys_summary_text.clone().into());
    window.set_private_keys_details_text(projection.private_keys_details_text.clone().into());
    window.set_private_keys_public_key_text(projection.private_keys_public_key_text.clone().into());
    window
        .set_private_keys_test_result_text(projection.private_keys_test_result_text.clone().into());
    window.set_private_keys_status_text(projection.private_keys_status_text.clone().into());
    window.set_private_keys_remove_confirm_visible(projection.private_keys_remove_confirm_visible);
    window.set_private_keys_deploy_confirm_visible(projection.private_keys_deploy_confirm_visible);
    window.set_private_keys_deploy_command_text(
        projection.private_keys_deploy_command_text.clone().into(),
    );
    window.set_private_keys_deploy_summary_text(
        projection.private_keys_deploy_summary_text.clone().into(),
    );
    window.set_private_keys_remove_warning_text(
        projection.private_keys_remove_warning_text.clone().into(),
    );
    if window.get_private_keys_import_label_text().as_str()
        != projection.private_keys_import_label_text
    {
        window.set_private_keys_import_label_text(
            projection.private_keys_import_label_text.clone().into(),
        );
    }
    if window.get_private_keys_import_path_text().as_str()
        != projection.private_keys_import_path_text
    {
        window.set_private_keys_import_path_text(
            projection.private_keys_import_path_text.clone().into(),
        );
    }
    if window.get_private_keys_import_passphrase_text().as_str()
        != projection.private_keys_import_passphrase_text
    {
        window.set_private_keys_import_passphrase_text(
            projection
                .private_keys_import_passphrase_text
                .clone()
                .into(),
        );
    }
    window.set_private_keys_remember_import_passphrase(
        projection.private_keys_remember_import_passphrase,
    );
    window.set_private_keys_import_enabled(projection.private_keys_import_enabled);
    if window.get_private_keys_passphrase_text().as_str() != projection.private_keys_passphrase_text
    {
        window.set_private_keys_passphrase_text(
            projection.private_keys_passphrase_text.clone().into(),
        );
    }
    window.set_private_keys_passphrase_stored(projection.private_keys_passphrase_stored);
    window.set_private_keys_passphrase_manage_enabled(
        projection.private_keys_passphrase_manage_enabled,
    );
    // N4：主机密钥页。
    window.set_host_keys_modal_visible(projection.host_keys_modal_visible);
    set_host_keys_groups_if_changed(window, &projection.host_keys_groups);
    window.set_host_keys_selected_group(projection.host_keys_selected_group);
    window.set_host_keys_selected_entry(projection.host_keys_selected_entry);
    window.set_host_keys_details_text(projection.host_keys_details_text.clone().into());
    window.set_host_keys_path_text(projection.host_keys_path_text.clone().into());
    window.set_host_keys_status_text(projection.host_keys_status_text.clone().into());
    window.set_host_keys_import_visible(projection.host_keys_import_visible);
    if window.get_host_keys_import_text().as_str() != projection.host_keys_import_text {
        window.set_host_keys_import_text(projection.host_keys_import_text.clone().into());
    }
    if window.get_host_keys_clear_confirmation_text().as_str()
        != projection.host_keys_clear_confirmation_text
    {
        window.set_host_keys_clear_confirmation_text(
            projection.host_keys_clear_confirmation_text.clone().into(),
        );
    }
    // N4：认证弹窗（服务端能力 + 已填内容 + 多轮挑战）。
    window.set_auth_prompt_visible(projection.auth_prompt_visible);
    window.set_auth_prompt_host_text(projection.auth_prompt_host_text.clone().into());
    window.set_auth_prompt_problem_text(projection.auth_prompt_problem_text.clone().into());
    window.set_auth_prompt_has_usable_method(projection.auth_prompt_has_usable_method);
    window.set_auth_prompt_selected_method(projection.auth_prompt_selected_method);
    window.set_auth_prompt_password_visible(projection.auth_prompt_password_visible);
    window.set_auth_prompt_publickey_visible(projection.auth_prompt_publickey_visible);
    window.set_auth_prompt_keyboard_visible(projection.auth_prompt_keyboard_visible);
    window.set_auth_prompt_submit_enabled(projection.auth_prompt_submit_enabled);
    if window.get_auth_prompt_password_text().as_str() != projection.auth_prompt_password_text {
        window.set_auth_prompt_password_text(projection.auth_prompt_password_text.clone().into());
    }
    window.set_auth_prompt_remember_password(projection.auth_prompt_remember_password);
    set_auth_key_options_if_changed(window, &projection.auth_prompt_key_options);
    window.set_auth_prompt_selected_key_index(projection.auth_prompt_selected_key_index);
    window.set_auth_prompt_selected_key_label(
        projection.auth_prompt_selected_key_label.clone().into(),
    );
    if window.get_auth_prompt_passphrase_text().as_str() != projection.auth_prompt_passphrase_text {
        window
            .set_auth_prompt_passphrase_text(projection.auth_prompt_passphrase_text.clone().into());
    }
    window.set_auth_prompt_use_agent(projection.auth_prompt_use_agent);
    window
        .set_auth_prompt_publickey_submit_enabled(projection.auth_prompt_publickey_submit_enabled);
    window.set_auth_prompt_challenge_name(projection.auth_prompt_challenge_name.clone().into());
    window.set_auth_prompt_challenge_instruction(
        projection.auth_prompt_challenge_instruction.clone().into(),
    );
    set_auth_challenge_prompts_if_changed(window, &projection.auth_prompt_challenge_prompts);
    window.set_auth_prompt_challenge_round(projection.auth_prompt_challenge_round);
    window.set_auth_prompt_challenge_round_total(projection.auth_prompt_challenge_round_total);
    window.set_auth_prompt_keyboard_final_round(projection.auth_prompt_keyboard_final_round);
    window.set_auth_prompt_keyboard_submit_enabled(projection.auth_prompt_keyboard_submit_enabled);
    restore_shortcut_focus_after_modal(window, projection);
}

/// 上一次投影时是否有模态弹窗可见（W5-A2：弹窗关闭后把键盘焦点还给全局快捷键 FocusScope）。
static MODAL_WAS_VISIBLE: AtomicBool = AtomicBool::new(false);

/// W5-A2：模态弹窗关闭时，被销毁的输入框会带走键盘焦点，窗口级快捷键随之失效；
/// 此时把焦点还给 `MainWindow` 里的全局快捷键 FocusScope。
///
/// 弹窗可见性一半来自投影（编辑器/known hosts），一半来自窗口本地属性（设置、关于、
/// 退出确认、删除确认、SFTP 弹窗），所以两边都要读。只在"可见 → 不可见"的跳变上、
/// 且终端没有持有焦点时抢焦点：终端聚焦时它的 FocusScope 会自己保住焦点，
/// 抢过来只会让终端失去键盘输入。
fn restore_shortcut_focus_after_modal(window: &MainWindow, projection: &AppProjection) {
    let modal_visible = projection.editor_modal_visible
        || projection.known_hosts_modal_visible
        || projection.password_prompt_visible
        || projection.private_keys_modal_visible
        || projection.host_keys_modal_visible
        || projection.auth_prompt_visible
        || window.get_settings_dialog_visible()
        || window.get_about_dialog_visible()
        || window.get_quit_dialog_visible()
        || window.get_delete_session_dialog_visible()
        || window.get_sftp_dialog_kind() != "none";
    if MODAL_WAS_VISIBLE.swap(modal_visible, Ordering::Relaxed)
        && !modal_visible
        && !window.get_terminal_focused()
    {
        window.invoke_focus_global_shortcuts();
    }
}

pub fn bootstrap_app() -> AppResult<YShellApp> {
    init_logging()?;
    let config_dir = yshell_config::discover_config_dir().map_err(AppError::from_error)?;
    Ok(YShellApp {
        state: AppState::new(config_dir)?,
    })
}

pub fn init_logging() -> AppResult<()> {
    yshell_logging::init_tracing(&yshell_logging::LoggingConfig::default())
        .map_err(AppError::from_error)
}

#[cfg(test)]
mod tests {
    use super::{
        platform_default_ui_font, resolve_appearance, resolve_language,
        resolve_startup_ssh_backend, resolve_ui_font, startup_ssh_backend_from_value,
        terminal_grid_cell, terminal_size_from_viewport,
    };
    use crate::runtime::AppRuntime;
    use tempfile::tempdir;
    use yshell_config::discover_config_dir;
    use yshell_ssh::TransportBackend;
    use yshell_ui::appearance::{self, ThemeMode};

    #[test]
    fn startup_ssh_backend_env_selects_native_ssh_runtime() {
        // 取值解析：大小写/空白不敏感，未知值回退默认（None → 原生 SSH）。
        assert_eq!(
            startup_ssh_backend_from_value(Some("native-ssh")),
            Some(TransportBackend::Real)
        );
        assert_eq!(
            startup_ssh_backend_from_value(Some(" Native-SSH ")),
            Some(TransportBackend::Real)
        );
        assert_eq!(
            startup_ssh_backend_from_value(Some("fake")),
            Some(TransportBackend::Fake)
        );
        assert_eq!(startup_ssh_backend_from_value(Some("bogus")), None);
        assert_eq!(startup_ssh_backend_from_value(Some("   ")), None);
        assert_eq!(startup_ssh_backend_from_value(None), None);

        // S2/D26：无环境变量启动 → 原生 SSH（不是 fake-shell 横幅）。
        // 同一测试内串行覆盖三种取值，避免并行测试互相改 `YSHELL_SSH_BACKEND`。
        std::env::remove_var("YSHELL_SSH_BACKEND");
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");

        let default_projection = resolve_startup_ssh_backend(&mut runtime);
        assert_eq!(default_projection.transport_backend_text, "native-ssh");
        assert_eq!(
            default_projection.sftp_listing_kind_text,
            "native-ssh-selected"
        );
        assert_eq!(
            default_projection.sftp_session_status_kind_text,
            "disconnected"
        );
        assert!(!default_projection.status_text.contains("fake"));
        assert!(!default_projection
            .sftp_listing_rows_text
            .contains("fake backend"));

        // e2e/离线测试开关仍可用：YSHELL_SSH_BACKEND=fake。
        std::env::set_var("YSHELL_SSH_BACKEND", "fake");
        let fake_projection = resolve_startup_ssh_backend(&mut runtime);
        assert_eq!(fake_projection.transport_backend_text, "fake");
        assert_eq!(
            fake_projection.sftp_listing_kind_text,
            "fake-backend-selected"
        );

        // 显式 native-ssh 与缺省一致。
        std::env::set_var("YSHELL_SSH_BACKEND", "native-ssh");
        let native_projection = resolve_startup_ssh_backend(&mut runtime);
        std::env::remove_var("YSHELL_SSH_BACKEND");
        assert_eq!(native_projection.transport_backend_text, "native-ssh");
        assert_eq!(
            native_projection.sftp_listing_kind_text,
            "native-ssh-selected"
        );
    }

    #[test]
    fn discovers_a_config_dir_in_this_environment() {
        let path = discover_config_dir().expect("config directory should be discoverable");
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn viewport_size_maps_to_terminal_columns_and_rows() {
        assert_eq!(
            terminal_size_from_viewport(900.0, 360.0, (9, 18), 1.0),
            (100, 20)
        );
    }

    #[test]
    fn viewport_size_uses_the_renderer_cell_size() {
        let renderer = yshell_terminal::TerminalRenderer::new();
        let cell_size = renderer.cell_size();
        let (columns, rows) = terminal_size_from_viewport(900.0, 360.0, cell_size, 1.0);
        // ceil 网格：位图必须覆盖整个视口（不许出现会被拉伸的空白条），
        // 且去掉最后一格后不再覆盖。
        assert!(u32::from(columns) * cell_size.0 >= 900);
        assert!(u32::from(columns - 1) * cell_size.0 < 900);
        assert!(u32::from(rows) * cell_size.1 >= 360);
        assert!(u32::from(rows - 1) * cell_size.1 < 360);
    }

    #[test]
    fn viewport_size_respects_minimum_terminal_dimensions() {
        assert_eq!(terminal_size_from_viewport(0.0, 0.0, (9, 18), 1.0), (20, 4));
    }

    #[test]
    fn viewport_size_uses_physical_pixels_for_non_unit_scale() {
        // 逻辑 900×360、scale=1.25、物理 cell 12×24 → 物理视口 1125×450。
        assert_eq!(
            terminal_size_from_viewport(900.0, 360.0, (12, 24), 1.25),
            (94, 19)
        );
        // scale=2.0：物理尺寸翻倍。
        assert_eq!(
            terminal_size_from_viewport(900.0, 360.0, (10, 19), 2.0),
            (180, 38)
        );
        // 非有限/非正 scale 视作 1.0。
        assert_eq!(
            terminal_size_from_viewport(900.0, 360.0, (10, 19), f32::NAN),
            (90, 19)
        );
    }

    #[test]
    fn grid_cell_mapping_uses_physical_pixels() {
        // scale=1：逻辑像素 = 物理像素。
        assert_eq!(
            terminal_grid_cell(9.0, 18.0, (9, 18), 1.0, 100, 20),
            Some((1, 1))
        );
        // scale=1.25、物理 cell 20：逻辑 10px = 物理 12.5px → 第 0 列；
        // 逻辑 16px = 物理 20px → 第 1 列。
        assert_eq!(
            terminal_grid_cell(10.0, 5.0, (20, 20), 1.25, 10, 10),
            Some((0, 0))
        );
        assert_eq!(
            terminal_grid_cell(16.0, 5.0, (20, 20), 1.25, 10, 10),
            Some((1, 0))
        );
        // 越界坐标 clamp 到最后一格；空网格返回 None。
        assert_eq!(
            terminal_grid_cell(9999.0, 9999.0, (20, 20), 1.25, 10, 10),
            Some((9, 9))
        );
        assert_eq!(terminal_grid_cell(0.0, 0.0, (20, 20), 1.25, 0, 10), None);
    }

    /// 双击兜底：重复点击窗口内再点同一节点算双击；超窗或换节点不算；激活去重
    /// 保证 `double-clicked` 与重复点击兜底不会把同一个节点打开两次。
    #[test]
    fn session_tree_repeat_click_and_activation_dedup() {
        use super::{
            session_tree_claim_activation, session_tree_register_click, SessionTreeInput,
            SESSION_TREE_ACTIVATION_DEDUP_WINDOW, SESSION_TREE_REPEAT_CLICK_WINDOW,
        };
        use std::cell::RefCell;
        use std::rc::Rc;
        use std::time::{Duration, Instant};

        let state = Rc::new(RefCell::new(SessionTreeInput::default()));

        // 第一次点击不是重复；窗口内同一节点是重复；换节点不是。
        assert!(!session_tree_register_click(&state, "saved-one"));
        assert!(session_tree_register_click(&state, "saved-one"));
        assert!(!session_tree_register_click(&state, "saved-two"));

        // 超过窗口（回填旧时间戳模拟）→ 不再算重复。
        state.borrow_mut().last_click = Some((
            "saved-two".to_owned(),
            Instant::now() - SESSION_TREE_REPEAT_CLICK_WINDOW - Duration::from_millis(50),
        ));
        assert!(!session_tree_register_click(&state, "saved-two"));

        // 激活去重：窗口内同一节点只认领一次，换节点立刻可认领，超窗后可再次认领。
        let state = Rc::new(RefCell::new(SessionTreeInput::default()));
        assert!(session_tree_claim_activation(&state, "saved-one"));
        assert!(!session_tree_claim_activation(&state, "saved-one"));
        assert!(session_tree_claim_activation(&state, "saved-two"));
        state.borrow_mut().last_activation = Some((
            "saved-two".to_owned(),
            Instant::now() - SESSION_TREE_ACTIVATION_DEDUP_WINDOW - Duration::from_millis(50),
        ));
        assert!(session_tree_claim_activation(&state, "saved-two"));
    }

    /// 选中变化不得替换会话树行模型：行的选中态由标量属性
    /// `saved_session_tree_selected_id` 驱动，换模型会重建行实例、重置 Slint 的
    /// 双击计数（双击退化成两次单击，会话打不开）。
    #[test]
    fn session_tree_row_comparison_ignores_selection_only_changes() {
        use super::{session_tree_row_matches, SessionTreeRow};

        let projected = |selected: bool, label: &str| crate::runtime::SessionTreeRow {
            kind: "session".to_owned(),
            depth: 1,
            label: label.to_owned(),
            detail: "ops@example.test".to_owned(),
            id: "saved-one".to_owned(),
            expanded: false,
            selected,
        };
        let modelled = |selected: bool| SessionTreeRow {
            kind: "session".into(),
            depth: 1,
            label: "One".into(),
            detail: "ops@example.test".into(),
            id: "saved-one".into(),
            expanded: false,
            selected,
        };

        // 只有 selected 不同（模型里还是旧值）→ 判定为"无需重建"。
        assert!(session_tree_row_matches(
            &modelled(false),
            &projected(true, "One")
        ));
        assert!(session_tree_row_matches(
            &modelled(true),
            &projected(true, "One")
        ));

        // 其它字段变化仍然触发重建。
        assert!(!session_tree_row_matches(
            &modelled(false),
            &projected(true, "Renamed")
        ));
        let mut expanded = projected(true, "One");
        expanded.expanded = true;
        assert!(!session_tree_row_matches(&modelled(false), &expanded));
    }

    #[test]
    fn appearance_defaults_to_system_and_windows_blue() {
        let (mode, accent) = resolve_appearance(None, None);
        assert_eq!(mode, ThemeMode::System);
        assert_eq!(accent, appearance::resolve_accent("blue"));
    }

    #[test]
    fn appearance_accepts_env_values_and_falls_back_on_unknown() {
        let (mode, accent) = resolve_appearance(Some("Dark"), Some("teal"));
        assert_eq!(mode, ThemeMode::Dark);
        assert_eq!(accent, appearance::resolve_accent("teal"));

        let (mode, accent) = resolve_appearance(Some("bogus"), Some("bogus"));
        assert_eq!(mode, ThemeMode::System);
        assert_eq!(accent, appearance::resolve_accent("blue"));
    }

    #[test]
    fn language_accepts_explicit_chinese_and_english_values() {
        assert_eq!(resolve_language("zh-CN", None), "zh-CN");
        assert_eq!(resolve_language("zh", None), "zh-CN");
        assert_eq!(resolve_language("zh-Hans", None), "zh-CN");
        assert_eq!(resolve_language("en-US", None), "en-US");
        assert_eq!(resolve_language("en", None), "en-US");
    }

    #[test]
    fn language_falls_back_to_the_system_locale() {
        assert_eq!(resolve_language("system", Some("zh_CN.UTF-8")), "zh-CN");
        assert_eq!(resolve_language("", Some("en_GB.UTF-8")), "en-US");
        assert_eq!(resolve_language("system", None), "en-US");
    }

    #[test]
    fn language_ignores_case_and_surrounding_whitespace() {
        assert_eq!(resolve_language("  EN-us ", None), "en-US");
    }

    #[test]
    fn ui_font_prefers_a_non_blank_override() {
        // 显式传参，不读真实环境变量，避免 CI 平台差异。
        assert_eq!(resolve_ui_font(Some("Inter")), "Inter");
        assert_eq!(resolve_ui_font(Some("  Noto Sans SC  ")), "Noto Sans SC");
    }

    #[test]
    fn ui_font_uses_the_platform_default_when_override_is_missing_or_blank() {
        // 不写死平台字体名，跨平台 CI 均可通过。
        assert_eq!(resolve_ui_font(None), platform_default_ui_font());
        assert_eq!(resolve_ui_font(Some("")), platform_default_ui_font());
        assert_eq!(resolve_ui_font(Some(" \t ")), platform_default_ui_font());
    }
}
