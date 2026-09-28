//! N7 multi-window manager: window lifecycle, per-window terminal surfaces and
//! the application-level timer.
//!
//! 设计：`docs/product/yshell-next-n7-multi-window.md`（D5–D8 已确认）。
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
use crate::error::{AppError, AppResult};
use crate::runtime::AppRuntime;
use crate::sftp_jobs::{SftpJobHandle, SftpJobMessage};

/// 应用级轮询间隔（沿用单窗口时代的 120ms）。
const POLL_INTERVAL: Duration = Duration::from_millis(120);

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
    /// 显隐/主题/对话框）会在下一个 tick（≤120ms）广播到所有窗口。
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
}

/// N7：窗口生命周期 + 应用级轮询的管理器。
pub(crate) struct WindowManager {
    runtime: Rc<RefCell<AppRuntime>>,
    windows: Vec<WindowEntry>,
    next_window_id: u64,
    dialog_tx: std::sync::mpsc::Sender<FileDialogOutcome>,
    dialog_rx: Receiver<FileDialogOutcome>,
    sftp_job_rx: Receiver<SftpJobMessage>,
    timer: Option<Timer>,
}

impl WindowManager {
    /// 创建管理器并打开初始窗口（主窗口 id = 0）。
    pub(crate) fn new(runtime: Rc<RefCell<AppRuntime>>) -> AppResult<Rc<RefCell<Self>>> {
        let (dialog_tx, dialog_rx) = std::sync::mpsc::channel();
        let (sftp_job_tx, sftp_job_rx) = crate::sftp_jobs::start_sftp_job_worker();
        runtime.borrow_mut().sftp_jobs = Some(SftpJobHandle::new(sftp_job_tx));
        let manager = Rc::new(RefCell::new(Self {
            runtime,
            windows: Vec::new(),
            next_window_id: 0,
            dialog_tx,
            dialog_rx,
            sftp_job_rx,
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
            // 语言必须在"第一个组件创建之后、窗口显示之前"选择：bundled translations
            // 挂在首个组件创建出来的全局上下文里（提前调用会报 NoTranslationsBundled，
            // 界面整体回退英文），而晚于 show() 会先闪一帧英文。重复调用是幂等的。
            apply_language();
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
            });
        }
        manager.borrow_mut().refresh_all();
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
        let (needs_confirm, is_last) = {
            let runtime = self.runtime.borrow();
            (
                runtime.active_connections_in_window(window_id) > 0,
                runtime.is_last_window(window_id),
            )
        };
        tracing::info!(
            target: "yshell::app",
            window_id,
            needs_confirm,
            is_last,
            "window close requested"
        );
        if !needs_confirm {
            self.close_window_now(window_id);
            return true;
        }
        if is_last {
            // D7 + 设计 §1：最后一个窗口关闭 = 退出，沿用既有 Quit 确认。
            if let Some(entry) = self.entry(window_id) {
                entry.window.set_quit_dialog_visible(true);
                entry.window.invoke_focus_top_dialog();
            }
            return false;
        }
        self.runtime
            .borrow_mut()
            .set_window_close_pending(window_id, true);
        self.refresh_window(window_id);
        if let Some(entry) = self.entry(window_id) {
            entry.window.invoke_focus_top_dialog();
        }
        false
    }

    /// 关闭窗口确认弹窗：确认后断开并关闭。
    pub(crate) fn confirm_close_window(&mut self, window_id: u64) {
        self.runtime
            .borrow_mut()
            .set_window_close_pending(window_id, false);
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
    pub(crate) fn note_window_activation(&mut self, window_id: u64) {
        self.runtime.borrow_mut().focus_window(window_id);
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
            // N0：所有终端输出（活动会话优先 + 游标轮转后台标签）。
            match runtime.poll_all_terminal_outputs() {
                Ok(Some(_)) => dirty = true,
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(target: "yshell::app", "terminal output poll failed: {error}");
                }
            }
        }

        // 每窗口视口尺寸 → 该窗口活动会话的 PTY resize。
        let focused = self.runtime.borrow().focused_window;
        for index in 0..self.windows.len() {
            let window_id = self.windows[index].id;
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
            let changed = {
                let Ok(mut runtime) = self.runtime.try_borrow_mut() else {
                    return;
                };
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
            if changed {
                dirty = true;
            }
            let entry = &self.windows[index];
            entry.surface.sync_scale_factor(&entry.window);
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
