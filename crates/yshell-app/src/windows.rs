//! N7 multi-window manager: window lifecycle, per-window terminal surfaces and
//! the application-level timer.
//!
//! 设计：`docs/product/archive/yshell-next-n7-multi-window.md`（D5–D8 已确认）。
//!
//! 结构：
//! * `AppRuntime` 单实例共享（UI 单线程）；窗口只持有"标签 UI 归属"，标签/会话/
//!   网格/日志句柄都在运行时里（迁移标签不重建 `SessionRuntime`）。
//! * 每个窗口一个 `MainWindow` + 一个 `TerminalSurface`（各自的渲染器/缩放/帧缓存）。
//! * 一个应用级 `Timer`：drain 文件对话框/SFTP worker 消息 → 轮询所有终端 →
//!   按各窗口视口同步 PTY 尺寸 → 逐窗口投影 + 重绘。
//! * 窗口关闭：带活动连接 → 确认后断开（D7）；最后一个窗口关闭 = 退出应用
//!   （沿用 Quit 确认文案）。
//!
//! 焦点：`WindowRuntime` 在每个 UI 回调 `borrow_mut()` 之前把该窗口标记为交互窗口，
//! 让 `active_tab_id`/`active_session_id` 始终指向用户正在操作的窗口；投影与渲染
//! 则通过 `projection_for_window` / `window_terminal_*` 显式解析窗口自己的活动会话。

use std::cell::{Ref, RefCell, RefMut};
use std::rc::{Rc, Weak};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use slint::CloseRequestResponse;
use slint::{ComponentHandle, Timer, TimerMode};
use yshell_terminal::{TerminalRenderer, DEFAULT_FONT_SIZE};

use crate::bootstrap::generated_ui::MainWindow;
use crate::bootstrap::{
    apply_appearance, apply_dialog_outcome, apply_fonts, apply_language, apply_projection,
    sanitize_scale_factor, terminal_size_from_viewport, wire_callbacks, FileDialogOutcome,
    TerminalSurface,
};
use crate::connect_jobs::ConnectJobMessage;
use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;
use crate::sftp_jobs::SftpJobMessage;

/// 应用级轮询间隔。
///
/// 原先 120ms 时，按键虽立即写入 SSH，但远端回显要等下一个 tick 才进网格，
/// 体感延迟明显高于 Xshell（约 ½ 个周期 ≈ 60ms，最坏 120ms）。16ms ≈ 60Hz，
/// 与常见终端刷新对齐；drain 文件对话框/SFTP 消息与 PTY 尺寸同步也走同一路径。
const POLL_INTERVAL: Duration = Duration::from_millis(16);

/// N7：每窗口的运行时句柄。
///
/// `borrow_mut`/`try_borrow_mut` 会先把该窗口标记为交互窗口（同步全局
/// `active_tab_id`/`active_session_id`），`borrow`/`try_borrow` 是纯读。
#[derive(Clone)]
pub(crate) struct WindowRuntime {
    runtime: Rc<RefCell<AppRuntime>>,
    window_id: u64,
}

impl WindowRuntime {
    pub(crate) fn new(runtime: Rc<RefCell<AppRuntime>>, window_id: u64) -> Rc<Self> {
        Rc::new(Self { runtime, window_id })
    }

    pub(crate) fn borrow(&self) -> Ref<'_, AppRuntime> {
        self.runtime.borrow()
    }

    /// 变更用的借用：先标记该窗口为交互窗口，再标记"UI 需要重投影"。
    ///
    /// `ui_dirty` 由应用级定时器消费：任何窗口的交互（含全局状态变更，例如面板
    /// 显隐/主题/对话框）会在下一个 tick（≤16ms）广播到所有窗口。
    pub(crate) fn borrow_mut(&self) -> RefMut<'_, AppRuntime> {
        let mut runtime = self.runtime.borrow_mut();
        runtime.focus_window(self.window_id);
        runtime.mark_ui_dirty();
        runtime
    }

    pub(crate) fn try_borrow(&self) -> Result<Ref<'_, AppRuntime>, std::cell::BorrowError> {
        self.runtime.try_borrow()
    }

    pub(crate) fn try_borrow_mut(
        &self,
    ) -> Result<RefMut<'_, AppRuntime>, std::cell::BorrowMutError> {
        let mut runtime = self.runtime.try_borrow_mut()?;
        runtime.focus_window(self.window_id);
        runtime.mark_ui_dirty();
        Ok(runtime)
    }
}

struct WindowEntry {
    id: u64,
    window: MainWindow,
    surface: TerminalSurface,
    /// 最近一次用于 PTY 尺寸同步的视口（列/行；0 = 尚未同步）。
    last_viewport: (u16, u16),
    /// WP4：次 pane 视口（未分屏时保持 (0,0)）。
    last_secondary_viewport: (u16, u16),
}

/// 点击窗口关闭按钮后的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowCloseAction {
    /// 直接关闭窗口（最后一个窗口 = 退出应用）。
    CloseNow,
    /// 最后一个窗口仍有活动连接：弹 Quit 确认。
    ConfirmQuit,
    /// 非最后一个窗口仍有活动连接：弹窗口关闭确认。
    ConfirmClose,
}

/// 计算关闭窗口的动作。
///
/// 「关闭会话前确认」只控制非最后窗口的关闭确认；最后一个窗口的退出确认由
/// `confirm_quit_app` 单独控制，不能被「不再询问关闭会话」顺手关掉。
fn window_close_action(
    has_active_connections: bool,
    is_last: bool,
    confirm_close_session: bool,
    confirm_quit_app: bool,
) -> WindowCloseAction {
    if is_last {
        if has_active_connections && confirm_quit_app {
            WindowCloseAction::ConfirmQuit
        } else {
            WindowCloseAction::CloseNow
        }
    } else if has_active_connections && confirm_close_session {
        WindowCloseAction::ConfirmClose
    } else {
        WindowCloseAction::CloseNow
    }
}

/// N7：窗口生命周期 + 应用级轮询的管理器。
pub(crate) struct WindowManager {
    runtime: Rc<RefCell<AppRuntime>>,
    windows: Vec<WindowEntry>,
    next_window_id: u64,
    dialog_tx: std::sync::mpsc::Sender<FileDialogOutcome>,
    dialog_rx: Receiver<FileDialogOutcome>,
    sftp_job_rx: Receiver<SftpJobMessage>,
    connect_job_rx: Receiver<ConnectJobMessage>,
    timer: Option<Timer>,
}

impl WindowManager {
    /// 创建管理器并打开初始窗口（主窗口 id = 0）。
    pub(crate) fn new(runtime: Rc<RefCell<AppRuntime>>) -> AppResult<Rc<RefCell<Self>>> {
        let (dialog_tx, dialog_rx) = std::sync::mpsc::channel();
        let (sftp_job_tx, sftp_job_rx, sftp_limiter) =
            crate::sftp_jobs::start_sftp_job_worker_with_limit(1);
        let (connect_jobs, connect_job_rx) = crate::connect_jobs::start_connect_job_worker();
        {
            let mut runtime = runtime.borrow_mut();
            runtime.sftp_jobs = Some(crate::sftp_jobs::SftpJobHandle::with_limiter(
                sftp_job_tx,
                sftp_limiter,
            ));
            runtime.connect_jobs = Some(connect_jobs);
            // 应用配置里的并行传输数立即生效（不必重启）。
            let limit = runtime.config_document.ui.sftp_max_parallel_transfers;
            runtime.set_sftp_parallel_transfer_limit(limit);
        }
        let manager = Rc::new(RefCell::new(Self {
            runtime,
            windows: Vec::new(),
            next_window_id: 0,
            dialog_tx,
            dialog_rx,
            sftp_job_rx,
            connect_job_rx,
            timer: None,
        }));
        Self::open_window(&manager, None)?;
        let weak: Weak<RefCell<Self>> = Rc::downgrade(&manager);
        let timer = Timer::default();
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || {
            let Some(manager) = weak.upgrade() else {
                return;
            };
            let Ok(mut manager) = manager.try_borrow_mut() else {
                return;
            };
            manager.tick();
        });
        manager.borrow_mut().timer = Some(timer);
        Ok(manager)
    }

    fn allocate_window_id(&mut self) -> u64 {
        let id = self.next_window_id;
        self.next_window_id += 1;
        id
    }

    /// 打开一个窗口：`window_id` 为 `None` 时开主窗口（登记 id 0 并聚焦）。
    fn open_window(manager: &Rc<RefCell<Self>>, window_id: Option<u64>) -> AppResult<()> {
        let (window_id, window, surface, window_runtime, manager_weak, dialog_tx) = {
            let mut this = manager.borrow_mut();
            let window_id = match window_id {
                Some(window_id) => window_id,
                None => {
                    let window_id = this.allocate_window_id();
                    this.runtime.borrow_mut().register_window(window_id);
                    window_id
                }
            };
            let window = MainWindow::new().map_err(AppError::from_error)?;
            // WIN-2/WIN-3：Windows 下标题栏拖动改走 Win32 原生移动循环（见
            // win_chrome::start_native_window_move）；非 Windows 继续用 WindowMoveArea。
            window.set_native_title_drag(cfg!(windows));
            // 语言必须在"第一个组件创建之后、窗口显示之前"选择：bundled translations
            // 挂在首个组件创建出来的全局上下文里（提前调用会报 NoTranslationsBundled，
            // 界面整体回退英文），而晚于 show() 会先闪一帧英文。重复调用是幂等的。
            let ui_language = this.runtime.borrow().ui_language().to_owned();
            apply_language(&ui_language);
            apply_appearance(&window);
            apply_fonts(&window);
            // W5-A2：窗口级快捷键依赖 root FocusScope 持有键盘焦点。
            window.invoke_focus_global_shortcuts();
            let renderer = Rc::new(RefCell::new(TerminalRenderer::with_scale_factor(
                DEFAULT_FONT_SIZE,
                sanitize_scale_factor(window.window().scale_factor()),
            )));
            let surface = TerminalSurface::new(Rc::clone(&this.runtime), window_id, renderer);
            let window_runtime = WindowRuntime::new(Rc::clone(&this.runtime), window_id);
            (
                window_id,
                window,
                surface,
                window_runtime,
                Rc::downgrade(manager),
                this.dialog_tx.clone(),
            )
        };

        wire_callbacks(
            &window,
            window_id,
            window_runtime,
            Rc::clone(manager),
            surface.clone(),
            dialog_tx,
        );
        // 首次投影 + 显示（窗口由 winit 在事件循环里真正创建）。
        let projection = {
            let this = manager.borrow();
            let runtime = this.runtime.borrow();
            let projection = runtime.projection_for_window(window_id);
            drop(runtime);
            projection
        };
        apply_projection(&window, &projection);
        surface.refresh(&window);
        window.show().map_err(AppError::from_error)?;
        surface.sync_scale_factor(&window);
        surface.refresh(&window);
        // Win11：无框窗口默认直角；DWM 圆角需显式 opt-in。HWND 可能要等事件循环
        // 首帧才就绪，因此 show 后立即试一次，并再排队到事件循环补一次。
        crate::win_chrome::apply_rounded_corners(window.window());
        {
            let weak = window.as_weak();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(window) = weak.upgrade() {
                    crate::win_chrome::apply_rounded_corners(window.window());
                }
            });
        }

        // X（窗口关闭按钮）：窗口级语义（带活动连接 → 确认后断开；最后一个窗口 = 退出）。
        //
        // 在 `show()` 之后注册：窗口适配器只在显示后存在，提前注册的处理器在部分
        // 后端上会跟窗口适配器的回调槽脱节（实测 `xdotool windowclose` 收不到）。
        {
            let manager_weak = manager_weak.clone();
            window.window().on_close_requested(move || {
                let Some(manager) = manager_weak.upgrade() else {
                    return CloseRequestResponse::HideWindow;
                };
                let Ok(mut manager) = manager.try_borrow_mut() else {
                    return CloseRequestResponse::KeepWindowShown;
                };
                if manager.request_close_window(window_id) {
                    CloseRequestResponse::HideWindow
                } else {
                    CloseRequestResponse::KeepWindowShown
                }
            });
        }

        {
            let mut this = manager.borrow_mut();
            this.windows.push(WindowEntry {
                id: window_id,
                window,
                surface,
                last_viewport: (0, 0),
                last_secondary_viewport: (0, 0),
            });
        }
        // 首次布局后立刻把视口尺寸同步进 PTY，避免首个 tick 前仍是默认 80×24，
        // 导致 htop 等全屏 TUI 按错误网格排版。
        {
            let mut this = manager.borrow_mut();
            let _ = this.sync_window_viewport_size(window_id);
            this.refresh_all();
        }
        Ok(())
    }

    /// 主菜单/右键菜单：把标签移动到一个新窗口。
    pub(crate) fn move_tab_to_new_window(
        manager: &Rc<RefCell<Self>>,
        tab_id: &str,
    ) -> AppResult<()> {
        let source = { manager.borrow().runtime.borrow().window_of_tab(tab_id) };
        let window_id = {
            let mut this = manager.borrow_mut();
            let window_id = this.allocate_window_id();
            let mut runtime = this.runtime.borrow_mut();
            runtime.register_window(window_id);
            if let Err(error) = runtime.move_tab_to_window(tab_id, window_id) {
                runtime.close_window_tabs(window_id);
                return Err(error);
            }
            window_id
        };
        if let Err(error) = Self::open_window(manager, Some(window_id)) {
            // 开窗失败：把标签退回主窗口，避免标签丢在无人显示的窗口里。
            let mut this = manager.borrow_mut();
            let main = this.runtime.borrow().main_window_id();
            if let Some(main) = main {
                let _ = this.runtime.borrow_mut().move_tab_to_window(tab_id, main);
            }
            let _ = this.runtime.borrow_mut().close_window_tabs(window_id);
            this.refresh_all();
            return Err(error);
        }
        manager.borrow_mut().close_empty_secondary_window(source);
        Ok(())
    }

    /// 标签右键：把标签移动到主窗口。
    pub(crate) fn move_tab_to_main_window(&mut self, tab_id: &str) -> AppResult<()> {
        let main = {
            let runtime = self.runtime.borrow();
            runtime
                .main_window_id()
                .ok_or_else(|| AppError::new("no main window is available"))?
        };
        let source = self.runtime.borrow().window_of_tab(tab_id);
        self.runtime.borrow_mut().move_tab_to_window(tab_id, main)?;
        self.refresh_all();
        self.close_empty_secondary_window(source);
        Ok(())
    }

    /// N7：标签迁出后，源窗口若已空且不是主窗口就关掉（空窗口没有意义）。
    ///
    /// 只在源窗口没有标签时触发；没有活动连接（0 个标签）因此不需要确认。
    fn close_empty_secondary_window(&mut self, source: Option<u64>) {
        let Some(source) = source else {
            return;
        };
        let (is_main, is_empty) = {
            let runtime = self.runtime.borrow();
            (
                runtime.main_window_id() == Some(source),
                runtime.window_tab_ids(source).is_empty(),
            )
        };
        if !is_main && is_empty {
            tracing::info!(target: "yshell::app", window_id = source, "closing emptied window");
            self.close_window_now(source);
        }
    }

    /// 用户点击窗口关闭按钮：返回 `true` = 直接关闭，`false` = 保持打开（等待确认）。
    pub(crate) fn request_close_window(&mut self, window_id: u64) -> bool {
        let (has_active_connections, is_last, confirm_close, confirm_quit) = {
            let runtime = self.runtime.borrow();
            (
                runtime.active_connections_in_window(window_id) > 0,
                runtime.is_last_window(window_id),
                runtime.confirm_close_session_enabled(),
                runtime.confirm_quit_app_enabled(),
            )
        };
        let action =
            window_close_action(has_active_connections, is_last, confirm_close, confirm_quit);
        tracing::info!(
            target: "yshell::app",
            window_id,
            ?action,
            is_last,
            "window close requested"
        );
        match action {
            WindowCloseAction::CloseNow => {
                self.close_window_now(window_id);
                true
            }
            WindowCloseAction::ConfirmQuit => {
                // D7 + 设计 §1：最后一个窗口关闭 = 退出，沿用既有 Quit 确认。
                if let Some(entry) = self.entry(window_id) {
                    entry.window.set_quit_dialog_visible(true);
                    entry.window.invoke_focus_top_dialog();
                }
                false
            }
            WindowCloseAction::ConfirmClose => {
                self.runtime
                    .borrow_mut()
                    .set_window_close_pending(window_id, true);
                self.refresh_window(window_id);
                if let Some(entry) = self.entry(window_id) {
                    entry.window.invoke_focus_top_dialog();
                }
                false
            }
        }
    }

    /// 关闭窗口确认弹窗：确认后断开并关闭（`dont_ask_again` 持久化偏好）。
    pub(crate) fn confirm_close_window(&mut self, window_id: u64, dont_ask_again: bool) {
        self.runtime
            .borrow_mut()
            .set_window_close_pending(window_id, false);
        if dont_ask_again {
            let _ = self.runtime.borrow_mut().set_confirm_close_session(false);
        }
        self.close_window_now(window_id);
    }

    pub(crate) fn cancel_close_window(&mut self, window_id: u64) {
        self.runtime
            .borrow_mut()
            .set_window_close_pending(window_id, false);
        self.refresh_window(window_id);
    }

    /// 关闭窗口：断开全部标签 → 移除窗口 → 最后一个窗口时退出事件循环。
    fn close_window_now(&mut self, window_id: u64) {
        let is_last = {
            let runtime = self.runtime.borrow();
            runtime.is_last_window(window_id)
        };
        tracing::info!(
            target: "yshell::app",
            window_id,
            is_last,
            "closing window and disconnecting its tabs"
        );
        self.runtime.borrow_mut().close_window_tabs(window_id);
        if let Some(position) = self.windows.iter().position(|entry| entry.id == window_id) {
            let entry = self.windows.remove(position);
            let _ = entry.window.hide();
        }
        if is_last {
            self.shutdown();
        } else {
            self.refresh_all();
        }
    }

    /// 退出：停日志 + 结束事件循环（窗口在 `run()` 收尾时统一 drop）。
    pub(crate) fn shutdown(&mut self) {
        self.timer = None;
        self.runtime.borrow_mut().stop_all_session_logging();
        let _ = slint::quit_event_loop();
    }

    /// 把窗口标记为交互窗口（键盘/窗口激活回调；不产生投影）。
    ///
    /// TERM-2：同时把该窗口的设备像素比写进 runtime，Settings 的 live preview
    /// 与真实终端用同一个 DPR 光栅化（多窗口跨不同 DPI 显示器时以聚焦窗口为准）。
    pub(crate) fn note_window_activation(&mut self, window_id: u64) {
        self.runtime.borrow_mut().focus_window(window_id);
        if let Some(scale_factor) = self
            .windows
            .iter()
            .find(|entry| entry.id == window_id)
            .map(|entry| entry.surface.scale_factor())
        {
            self.runtime.borrow_mut().note_ui_scale_factor(scale_factor);
        }
    }

    /// 在指定窗口的状态栏写一条纯文本提示（错误路径用；窗口不存在时忽略）。
    pub(crate) fn set_window_status(&self, window_id: u64, text: &str) {
        if let Some(entry) = self.windows.iter().find(|entry| entry.id == window_id) {
            crate::bootstrap::set_plain_status(&entry.window, text.into());
        }
    }

    fn entry(&mut self, window_id: u64) -> Option<&mut WindowEntry> {
        self.windows.iter_mut().find(|entry| entry.id == window_id)
    }

    /// 重投影单个窗口（标签迁移、确认弹窗等）。
    pub(crate) fn refresh_window(&mut self, window_id: u64) {
        let Some(position) = self.windows.iter().position(|entry| entry.id == window_id) else {
            return;
        };
        let projection = self.runtime.borrow().projection_for_window(window_id);
        let entry = &self.windows[position];
        apply_projection(&entry.window, &projection);
        entry.surface.refresh(&entry.window);
    }

    /// 重投影所有窗口（任何全局变更后调用；每窗口解析自己的活动会话）。
    pub(crate) fn refresh_all(&mut self) {
        let window_ids: Vec<u64> = self.windows.iter().map(|entry| entry.id).collect();
        for window_id in window_ids {
            self.refresh_window(window_id);
        }
    }

    /// 把单个窗口的视口尺寸同步到其活动会话的 PTY。
    ///
    /// 必须先 `sync_scale_factor`：cell 度量按物理像素，网格用 viewport×scale/cell；
    /// 若先算后 sync，DPR 变化当帧会用旧 cell 量新视口，位图与 1phx 显示会错一拍。
    /// 返回 `true` 表示缩放或行列发生了变化（需要重投影/重绘）。
    fn sync_window_viewport_size(&mut self, window_id: u64) -> bool {
        let Some(index) = self.windows.iter().position(|entry| entry.id == window_id) else {
            return false;
        };
        let mut dirty = false;
        if self.windows[index]
            .surface
            .sync_scale_factor(&self.windows[index].window)
        {
            dirty = true;
        }
        // TERM-2：聚焦窗口（启动早期尚未有聚焦窗口时取首个窗口）的 DPR 是
        // Settings live preview 的渲染基准；`sync_scale_factor` 之后读取，
        // 保证预览与终端用同一帧的新 cell 度量。
        let scale_factor = self.windows[index].surface.scale_factor();
        let (columns, rows) = {
            let entry = &self.windows[index];
            terminal_size_from_viewport(
                entry.window.get_terminal_viewport_width_px(),
                entry.window.get_terminal_viewport_height_px(),
                entry.surface.cell_size(),
                entry.surface.scale_factor(),
            )
        };
        if self.windows[index].last_viewport != (columns, rows) {
            self.windows[index].last_viewport = (columns, rows);
            dirty = true;
        }
        let focused = self.runtime.borrow().focused_window;
        let changed = {
            let Ok(mut runtime) = self.runtime.try_borrow_mut() else {
                return dirty;
            };
            if focused.is_none() || focused == Some(window_id) {
                runtime.note_ui_scale_factor(scale_factor);
            }
            // TAB-4：记住聚焦窗口的视口尺寸，供 `+` 新建本地终端时作为 PTY 初始尺寸。
            if focused == Some(window_id) {
                runtime.note_terminal_viewport_size(columns, rows);
            }
            if focused == Some(window_id) {
                // 沿用单窗口时代的路径（聚焦窗口的 resize 会写状态栏）。
                matches!(
                    runtime.sync_active_terminal_size_passive(columns, rows),
                    Ok(Some(_))
                )
            } else {
                runtime
                    .sync_window_terminal_size(window_id, columns, rows, false)
                    .unwrap_or(false)
            }
        };
        // WP4：次 pane 视口 → 非聚焦会话 PTY。
        let secondary_changed = {
            let entry = &self.windows[index];
            if !entry.window.get_workspace_split_active() {
                self.windows[index].last_secondary_viewport = (0, 0);
                false
            } else {
                let (sec_cols, sec_rows) = terminal_size_from_viewport(
                    entry.window.get_terminal_viewport_secondary_width_px(),
                    entry.window.get_terminal_viewport_secondary_height_px(),
                    entry.surface.cell_size(),
                    entry.surface.scale_factor(),
                );
                if self.windows[index].last_secondary_viewport == (sec_cols, sec_rows) {
                    false
                } else {
                    self.windows[index].last_secondary_viewport = (sec_cols, sec_rows);
                    let Ok(mut runtime) = self.runtime.try_borrow_mut() else {
                        return dirty || changed;
                    };
                    runtime
                        .sync_window_secondary_terminal_size(window_id, sec_cols, sec_rows)
                        .unwrap_or(false)
                }
            }
        };
        dirty || changed || secondary_changed
    }

    /// 应用级轮询：消息 drain → 终端轮询 → 各窗口尺寸同步 → 投影 + 重绘。
    fn tick(&mut self) {
        let mut dirty = false;
        {
            let Ok(mut runtime) = self.runtime.try_borrow_mut() else {
                return;
            };
            // N7：窗口回调里的任何交互（含全局状态变更）都会置位脏标记——
            // 在这里一次性广播到所有窗口，保证双窗口的全局状态最终一致。
            if runtime.take_ui_dirty() {
                dirty = true;
            }
            // N4/N1：worker 线程消息回填（对话框结果、传输进度/冲突）。
            while let Ok(outcome) = self.dialog_rx.try_recv() {
                let _ = apply_dialog_outcome(&mut runtime, outcome);
                dirty = true;
            }
            while let Ok(message) = self.sftp_job_rx.try_recv() {
                let _ = runtime.apply_sftp_job_message(message);
                dirty = true;
            }
            while let Ok(message) = self.connect_job_rx.try_recv() {
                let _ = runtime.apply_connect_job_message(message);
                dirty = true;
            }
            // NET-1：代理管理器 "Test" 的后台探测结果。
            if runtime.poll_proxy_test_result() {
                dirty = true;
            }
            // 标签切换故意跳过同步 SFTP list_dir；这里补做首次/未缓存会话的列目录。
            if runtime.drain_pending_sftp_listing().is_some() {
                dirty = true;
            }
            // N0：所有终端输出（活动会话优先 + 游标轮转后台标签）。
            match runtime.poll_all_terminal_outputs() {
                Ok(Some(_)) => dirty = true,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(target: "yshell::app", "terminal output poll failed: {error}");
                }
            }
            // SFTP 标签的连接探活：libssh2 的 `authenticated()` 在 TCP 断开后仍
            // 然是 true，只有一次真实的 SFTP 操作失败才能证明连接没了。
            if runtime.poll_sftp_connection() {
                dirty = true;
            }
            // N2：QC 页的"最近使用"是相对时间文案（投影时拼装）：到期就重投影
            // 一次，否则文案会一直停在 "just now"。
            if runtime.quick_connect_relative_time_due() {
                dirty = true;
            }
            // N2：远端文件下载中：转圈相位按刷新推进（加载指示器）。
            if runtime.pending_remote_edit.is_some() {
                dirty = true;
            }
        }

        let requested_close = self
            .runtime
            .try_borrow_mut()
            .ok()
            .and_then(|mut runtime| runtime.take_requested_window_close());
        if let Some(window_id) = requested_close {
            if self.entry(window_id).is_some() {
                // Closing the last tab behaves like clicking the window close
                // button (confirm/quit flow included).
                self.request_close_window(window_id);
            }
            return;
        }

        let window_ids: Vec<u64> = self.windows.iter().map(|entry| entry.id).collect();
        for window_id in window_ids {
            if self.sync_window_viewport_size(window_id) {
                dirty = true;
            }
        }

        if dirty {
            self.refresh_all();
        }
        for index in 0..self.windows.len() {
            let entry = &self.windows[index];
            entry.surface.refresh(&entry.window);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{window_close_action, WindowCloseAction};

    #[test]
    fn quitting_the_last_window_still_honours_the_quit_setting() {
        // 「不再询问关闭会话」不能顺带关掉退出确认。
        assert_eq!(
            window_close_action(true, true, false, true),
            WindowCloseAction::ConfirmQuit
        );
        assert_eq!(
            window_close_action(true, true, true, true),
            WindowCloseAction::ConfirmQuit
        );
        // 用户明确关闭退出确认后直接退出。
        assert_eq!(
            window_close_action(true, true, false, false),
            WindowCloseAction::CloseNow
        );
        // 没有活动连接时不需要退出确认（与既有行为一致）。
        assert_eq!(
            window_close_action(false, true, true, true),
            WindowCloseAction::CloseNow
        );
    }

    #[test]
    fn close_confirmation_applies_only_to_non_last_windows() {
        assert_eq!(
            window_close_action(true, false, true, true),
            WindowCloseAction::ConfirmClose
        );
        assert_eq!(
            window_close_action(true, false, false, true),
            WindowCloseAction::CloseNow
        );
        // 退出设置不影响普通窗口的关闭确认。
        assert_eq!(
            window_close_action(true, false, true, false),
            WindowCloseAction::ConfirmClose
        );
        assert_eq!(
            window_close_action(false, false, true, true),
            WindowCloseAction::CloseNow
        );
    }
}
