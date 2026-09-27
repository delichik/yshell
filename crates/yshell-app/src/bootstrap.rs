//! Startup and app-shell bootstrap logic.

use std::{
    cell::Cell,
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use copypasta::{ClipboardContext, ClipboardProvider};
use slint::private_unstable_api::re_exports::ColorScheme;
use slint::{
    ComponentHandle, Model, ModelRc, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel,
};
use yshell_ssh::TransportBackend;
use yshell_terminal::TerminalRenderer;
use yshell_ui::appearance::{self, AccentColors, ThemeMode};

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::runtime::{AppProjection, AppRuntime, SftpCrumbData, SftpRowData};

mod generated_ui {
    #![allow(dead_code)]
    slint::include_modules!();
}

use generated_ui::{
    MainWindow, Palette, SessionTreeRow, SftpCrumb, SftpRow, TerminalScrollInfo, Theme,
};

/// Keeps the pixel-rendered terminal surface in sync with the runtime.
///
/// This is cloned into every UI callback; `refresh` is cheap when the frame
/// revision did not change.
#[derive(Clone)]
struct TerminalSurface {
    runtime: Rc<RefCell<AppRuntime>>,
    renderer: Rc<RefCell<TerminalRenderer>>,
    last_frame: Rc<RefCell<Option<(String, u64)>>>,
}

impl TerminalSurface {
    fn new(runtime: Rc<RefCell<AppRuntime>>, renderer: Rc<RefCell<TerminalRenderer>>) -> Self {
        Self {
            runtime,
            renderer,
            last_frame: Rc::new(RefCell::new(None)),
        }
    }

    fn cell_size(&self) -> (u32, u32) {
        self.renderer.borrow().cell_size()
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
        let renderer = Rc::new(RefCell::new(TerminalRenderer::new()));
        let surface = TerminalSurface::new(Rc::clone(&runtime), renderer);
        let initial_projection = resolve_startup_ssh_backend(&mut runtime.borrow_mut());
        apply_projection(&window, &initial_projection);
        surface.refresh(&window);
        wire_callbacks(&window, Rc::clone(&runtime), surface.clone());
        let _terminal_poll_timer = start_terminal_poll_timer(&window, runtime, surface);
        window.run().map_err(AppError::from_error)
    }
}

/// E2E/开发开关：`YSHELL_SSH_BACKEND=native-ssh|fake` 启动即切换传输后端，
/// 等价于点击 View 菜单的 `select_native_ssh_backend()` / `select_fake_backend()`。
/// 未设置或值无法识别时保持 desktop-startup 默认（fake）。
fn resolve_startup_ssh_backend(runtime: &mut AppRuntime) -> AppProjection {
    let startup_projection = runtime.prepare_desktop_startup_projection();
    match startup_ssh_backend_from_value(std::env::var("YSHELL_SSH_BACKEND").ok().as_deref()) {
        Some(TransportBackend::Real) => runtime.select_native_ssh_transport_backend(),
        Some(TransportBackend::Fake) => runtime.select_fake_transport_backend(),
        None => startup_projection,
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
    // 注意：`Palette.color-scheme` 在 Slint 1.9 没有公开的 Rust 类型，这里用生成代码
    // 同款的 re-export；W3 用自绘控件替换 std-widgets 后即可删除这段。
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

fn wire_callbacks(window: &MainWindow, runtime: Rc<RefCell<AppRuntime>>, surface: TerminalSurface) {
    // Never shadow this binding: each callback clones a fresh handle.
    let surface_source = surface;
    let clipboard = Rc::new(RefCell::new(ClipboardContext::new().ok()));

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_quick_connect(move |input| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let result = runtime_ref
            .borrow_mut()
            .handle_quick_connect(input.as_ref());
        match result {
            Ok(projection) => {
                apply_projection(&window, &projection);
                surface.refresh(&window);
            }
            Err(error) => {
                window.set_status_text(format!("Quick Connect error: {error}").into());
                window.set_status_kind_text("quick-connect-error".into());
                window.set_status_param_1_text(error.to_string().into());
                window.set_status_param_2_text("".into());
            }
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_new_session(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().handle_new_session();
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
            Err(error) => {
                set_plain_status(&window, format!("Toggle folder error: {error}").into())
            }
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
        let Some(result) = session_tree_activate(&tree_input_ref, &runtime_ref, id.as_ref())
        else {
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
    window.on_select_fake_backend(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref.borrow_mut().select_fake_transport_backend();
            apply_projection(&window, &projection);
            surface.refresh(&window);
        }
    });

    let weak = window.as_weak();
    let runtime_ref = Rc::clone(&runtime);
    let surface = surface_source.clone();
    window.on_select_native_ssh_backend(move || {
        if let Some(window) = weak.upgrade() {
            let projection = runtime_ref
                .borrow_mut()
                .select_native_ssh_transport_backend();
            apply_projection(&window, &projection);
            surface.refresh(&window);
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

    window.on_quit_app(move || {
        let _ = slint::quit_event_loop();
    });
}

fn start_terminal_poll_timer(
    window: &MainWindow,
    runtime: Rc<RefCell<AppRuntime>>,
    surface: TerminalSurface,
) -> Timer {
    let timer = Timer::default();
    let weak = window.as_weak();
    timer.start(TimerMode::Repeated, Duration::from_millis(120), move || {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let (columns, rows) = terminal_size_from_viewport(
            window.get_terminal_viewport_width_px(),
            window.get_terminal_viewport_height_px(),
            surface.cell_size(),
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
            match runtime.poll_active_terminal_output_passive() {
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

fn terminal_size_from_viewport(width_px: f32, height_px: f32, cell_size: (u32, u32)) -> (u16, u16) {
    const MIN_COLUMNS: u16 = 20;
    const MIN_ROWS: u16 = 4;

    let cell_width = cell_size.0.max(1) as f32;
    let cell_height = cell_size.1.max(1) as f32;
    let columns = ((width_px.max(0.0) / cell_width).floor() as u16).max(MIN_COLUMNS);
    let rows = ((height_px.max(0.0) / cell_height).floor() as u16).max(MIN_ROWS);
    (columns, rows)
}

/// Convert a pixel position on the terminal surface into a grid cell plus the
/// absolute scrollback-aware line.
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
    if metrics.columns == 0 || metrics.rows == 0 {
        return None;
    }
    let (cell_width, cell_height) = surface.cell_size();
    let column = ((x.max(0.0) / cell_width.max(1) as f32).floor() as u32)
        .min(u32::from(metrics.columns - 1));
    let viewport_row =
        ((y.max(0.0) / cell_height.max(1) as f32).floor() as u32).min(u32::from(metrics.rows - 1));
    let absolute_row = metrics.top_absolute_row + viewport_row as usize;
    Some((column as u16, u16::try_from(absolute_row).ok()?))
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

fn sftp_row_from(row: &SftpRowData) -> SftpRow {
    SftpRow {
        name: row.name.clone().into(),
        kind_text: row.kind_text.clone().into(),
        size_text: row.size_text.clone().into(),
        modified_text: row.modified_text.clone().into(),
        permissions_text: row.permissions_text.clone().into(),
        is_dir: row.is_dir,
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
    let duplicate = state
        .last_activation
        .as_ref()
        .is_some_and(|(last_id, at)| {
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
    window.set_tab_name_text(projection.tab_name_text.clone().into());
    window.set_tab_state_text(projection.tab_state_text.clone().into());
    window.set_tab_has_session(projection.tab_has_session);
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
    window.set_tunnels_summary_kind_text(projection.tunnels_summary_kind_text.clone().into());
    window.set_tunnels_summary_rows_text(projection.tunnels_summary_rows_text.clone().into());
    window.set_status_text(projection.status_text.clone().into());
    window.set_status_kind_text(projection.status_kind.clone().into());
    window.set_status_param_1_text(projection.status_param_1.clone().into());
    window.set_status_param_2_text(projection.status_param_2.clone().into());
    window.set_transport_backend_text(projection.transport_backend_text.clone().into());
    window.set_sftp_visible(projection.sftp_visible);
    window.set_tunnels_visible(projection.tunnels_visible);
    window.set_commands_visible(projection.commands_visible);
    window.set_app_version_text(projection.app_version_text.clone().into());
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
        terminal_size_from_viewport,
    };
    use crate::runtime::AppRuntime;
    use tempfile::tempdir;
    use yshell_config::discover_config_dir;
    use yshell_ssh::TransportBackend;
    use yshell_ui::appearance::{self, ThemeMode};

    #[test]
    fn startup_ssh_backend_env_selects_native_ssh_runtime() {
        // 取值解析：大小写/空白不敏感，未知值回退默认（None → desktop-startup fake）。
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

        // 真实读取环境变量后启动：投影立即是 native-ssh（等价于菜单切换）。
        std::env::set_var("YSHELL_SSH_BACKEND", "native-ssh");
        let temp = tempdir().expect("tempdir");
        let mut runtime = AppRuntime::new(temp.path().to_path_buf()).expect("runtime");
        let projection = resolve_startup_ssh_backend(&mut runtime);
        std::env::remove_var("YSHELL_SSH_BACKEND");
        assert_eq!(projection.transport_backend_text, "native-ssh");
        assert_eq!(projection.sftp_listing_kind_text, "native-ssh-selected");
    }

    #[test]
    fn discovers_a_config_dir_in_this_environment() {
        let path = discover_config_dir().expect("config directory should be discoverable");
        assert!(!path.as_os_str().is_empty());
    }

    #[test]
    fn viewport_size_maps_to_terminal_columns_and_rows() {
        assert_eq!(
            terminal_size_from_viewport(900.0, 360.0, (9, 18)),
            (100, 20)
        );
    }

    #[test]
    fn viewport_size_uses_the_renderer_cell_size() {
        let renderer = yshell_terminal::TerminalRenderer::new();
        let cell_size = renderer.cell_size();
        let (columns, rows) = terminal_size_from_viewport(900.0, 360.0, cell_size);
        assert!(u32::from(columns) * cell_size.0 <= 900);
        assert!(u32::from(rows) * cell_size.1 <= 360);
    }

    #[test]
    fn viewport_size_respects_minimum_terminal_dimensions() {
        assert_eq!(terminal_size_from_viewport(0.0, 0.0, (9, 18)), (20, 4));
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
