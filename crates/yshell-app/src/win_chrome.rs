//! Windows 自绘窗口装饰：Win11 圆角、不透明客户区、原生标题栏拖动等与 OS 相关的轻量适配。

use crate::error::AppResult;

/// 等待 winit 惰性创建原生窗口的轮询间隔（约一帧）。
const AUX_WINDOW_READY_POLL: std::time::Duration = std::time::Duration::from_millis(16);

/// 将窗口钉成不可缩放（独立弹窗 / 工具窗口）。
///
/// Slint 的 `Window` 没有公开 `resizable` 属性；通过 winit accessor 关闭缩放。
/// 非 winit / 取不到句柄时为 no-op。
pub fn set_resizable(window: &slint::Window, resizable: bool) {
    use slint::winit_030::WinitWindowAccessor;

    let _ = window.with_winit_window(|winit_window| {
        winit_window.set_resizable(resizable);
    });
}

/// 独立弹窗 show 后收尾：Win11 圆角 + 不可缩放。
pub fn prepare_aux_window(window: &slint::Window) {
    apply_rounded_corners(window);
    set_resizable(window, false);
}

/// 把 aux 设为 owner 的 owned popup，并隐藏任务栏按钮。
///
/// `show()` 之后调用；HWND 未就绪时可在下一帧再调一次。非 Windows 为 no-op。
pub fn attach_owned_popup(aux: &slint::Window, owner: &slint::Window) {
    #[cfg(windows)]
    attach_owned_popup_windows(aux, owner);
    #[cfg(not(windows))]
    {
        let _ = (aux, owner);
    }
}

/// 启用 / 禁用窗口输入（模态：disable owner；关闭后 re-enable）。
pub fn set_window_enabled(window: &slint::Window, enabled: bool) {
    #[cfg(windows)]
    set_window_enabled_windows(window, enabled);
    #[cfg(not(windows))]
    {
        let _ = (window, enabled);
    }
}

/// 把模态子窗按 owner 的客户区居中（Windows；非 Windows no-op）。
///
/// 全部使用物理像素：owner 客户区原点由 `ClientToScreen` 换算，客户区尺寸来自
/// `GetClientRect`，子窗尺寸来自 `GetWindowRect`。因此高 DPI 缩放下不需要再做
/// 额外换算，多显示器时也天然跟随 owner 所在显示器居中。
///
/// `show()` 刚返回时 winit 可能还没把首选尺寸落到实处，调用方在事件循环下一轮
/// 再调一次即可（幂等）。
pub fn center_aux_window(aux: &slint::Window, owner: &slint::Window) {
    #[cfg(windows)]
    center_aux_window_windows(aux, owner);
    #[cfg(not(windows))]
    {
        let _ = (aux, owner);
    }
}

/// 把前台与键盘焦点还给主窗口（模态子窗关闭后调用）。
///
/// Windows 上 owned 子窗隐藏后系统不保证把前台交还 owner；显式
/// `BringWindowToTop` + `SetForegroundWindow` 恢复到父窗。键盘焦点本身由 Slint
/// 管（调用方随后 `invoke_focus_global_shortcuts`）。非 Windows no-op。
pub fn focus_window(window: &slint::Window) {
    #[cfg(windows)]
    focus_window_windows(window);
    #[cfg(not(windows))]
    {
        let _ = window;
    }
}

/// 子窗 `show()` 后，winit 在事件循环里惰性创建原生窗口；因此用短定时器在后续
/// 事件循环轮次补一次圆角 / 不可缩放收尾，直到 HWND 就绪（最多 `attempts_left` 轮）。
pub fn prepare_aux_window_when_ready<W: slint::ComponentHandle + 'static>(
    weak: slint::Weak<W>,
    attempts_left: u8,
) {
    slint::Timer::single_shot(AUX_WINDOW_READY_POLL, move || {
        let Some(aux) = weak.upgrade() else {
            return;
        };
        prepare_aux_window(aux.window());
        if !window_hwnd_ready(aux.window()) && attempts_left > 0 {
            prepare_aux_window_when_ready(aux.as_weak(), attempts_left - 1);
        }
    });
}

/// 同上，但用于有 owner 的模态子窗：等 HWND 就绪后补 owner 关系与居中摆放。
///
/// `show()` 返回时 HWND 通常还不存在（winit 在事件循环里惰性创建），直接调
/// `attach_owned_popup` / `center_aux_window` 会静默 no-op；这里按事件循环轮次
/// 重试直到句柄可用。
pub fn finish_owned_aux_window_when_ready<W, O>(
    aux: slint::Weak<W>,
    owner: slint::Weak<O>,
    attempts_left: u8,
) where
    W: slint::ComponentHandle + 'static,
    O: slint::ComponentHandle + 'static,
{
    slint::Timer::single_shot(AUX_WINDOW_READY_POLL, move || {
        let (Some(aux), Some(owner)) = (aux.upgrade(), owner.upgrade()) else {
            return;
        };
        prepare_aux_window(aux.window());
        attach_owned_popup(aux.window(), owner.window());
        center_aux_window(aux.window(), owner.window());
        let ready = window_hwnd_ready(aux.window()) && window_hwnd_ready(owner.window());
        if !ready && attempts_left > 0 {
            finish_owned_aux_window_when_ready(aux.as_weak(), owner.as_weak(), attempts_left - 1);
        }
    });
}

/// 模态子窗在 owner 客户区里的居中左上角（物理像素）。
///
/// 全部输入输出都是物理像素，因此 DPI 缩放下不需要额外换算；多显示器时
/// `owner_origin` 已带上 owner 所在显示器的屏幕坐标。
/// 子窗比 owner 客户区还大时贴齐 owner 客户区左上角（`.max(0)`），避免把标题栏
/// 摆到屏幕外。
#[cfg(any(windows, test))]
fn centered_aux_origin(
    owner_origin: (i32, i32),
    owner_client_size: (i32, i32),
    aux_size: (i32, i32),
) -> (i32, i32) {
    let x = owner_origin.0 + ((owner_client_size.0 - aux_size.0) / 2).max(0);
    let y = owner_origin.1 + ((owner_client_size.1 - aux_size.1) / 2).max(0);
    (x, y)
}

/// 原生窗口句柄与尺寸是否已就绪（非 Windows 视为始终就绪）。
fn window_hwnd_ready(window: &slint::Window) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

        let Some(hwnd) = hwnd_of(window) else {
            return false;
        };
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        #[allow(unsafe_code)]
        let sized = unsafe { GetWindowRect(hwnd, &mut rect) != 0 };
        sized && rect.right > rect.left && rect.bottom > rect.top
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        true
    }
}

/// 原生 `HTCAPTION` 移动循环结束后，清掉 Slint 残留的指针抓取 / 按下状态。
///
/// 系统移动循环会吞掉 mouse-up，Slint 的 `MouseInputState` 仍把标题栏
/// `TitleBarDragTouch` 当作抓取者；下一次按下会被这个残留抓取消费并改写成
/// 合成 Moved（表现为“拖动后第一次点击按钮不生效，要点第二次”）。这里在事件
/// 循环的下一轮补发一次 `PointerExited`，让核心按 Cancel 清掉 grab/pressed，
/// 之后第一次按下就能正常命中控件。
///
/// 必须异步执行：`process_mouse_input` 在输入回调返回后才写回新的
/// `MouseInputState`，在回调里同步嵌套派发会被外层状态覆盖。
pub fn reset_pointer_state_after_native_move<T: slint::ComponentHandle + 'static>(
    weak: slint::Weak<T>,
) {
    // 0ms 单次定时器：在当前输入回调返回、Slint 写回新的 MouseInputState 之后再清。
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        let Some(component) = weak.upgrade() else {
            return;
        };
        let _ = component
            .window()
            .dispatch_event_with_result(slint::platform::WindowEvent::PointerExited);
    });
}

#[cfg(windows)]
fn center_aux_window_windows(aux: &slint::Window, owner: &slint::Window) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetClientRect, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
    };

    let (Some(owner_hwnd), Some(aux_hwnd)) = (hwnd_of(owner), hwnd_of(aux)) else {
        return;
    };

    #[allow(unsafe_code)]
    unsafe {
        let mut client = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetClientRect(owner_hwnd, &mut client) == 0 {
            return;
        }
        let mut origin = POINT { x: 0, y: 0 };
        if ClientToScreen(owner_hwnd, &mut origin) == 0 {
            return;
        }
        let mut aux_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(aux_hwnd, &mut aux_rect) == 0 {
            return;
        }
        let client_width = client.right - client.left;
        let client_height = client.bottom - client.top;
        let aux_width = aux_rect.right - aux_rect.left;
        let aux_height = aux_rect.bottom - aux_rect.top;
        if aux_width <= 0 || aux_height <= 0 {
            return;
        }
        let (x, y) = centered_aux_origin(
            (origin.x, origin.y),
            (client_width, client_height),
            (aux_width, aux_height),
        );
        SetWindowPos(
            aux_hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(windows)]
fn focus_window_windows(window: &slint::Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};

    let Some(hwnd) = hwnd_of(window) else {
        return;
    };
    // 模态结束仍在本进程内，前台锁定不阻止 SetForegroundWindow。
    #[allow(unsafe_code)]
    unsafe {
        BringWindowToTop(hwnd);
        SetForegroundWindow(hwnd);
    }
}

#[cfg(windows)]
fn hwnd_of(window: &slint::Window) -> Option<windows_sys::Win32::Foundation::HWND> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::Win32::Foundation::HWND;

    let mut out: Option<HWND> = None;
    let _ = window.with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        out = Some(win32.hwnd.get() as HWND);
    });
    out
}

#[cfg(windows)]
fn attach_owned_popup_windows(aux: &slint::Window, owner: &slint::Window) {
    use slint::winit_030::winit::platform::windows::WindowExtWindows;
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowLongPtrW, GWLP_HWNDPARENT};

    let Some(owner_hwnd) = hwnd_of(owner) else {
        return;
    };
    let Some(aux_hwnd) = hwnd_of(aux) else {
        return;
    };

    // Owned windows do not get a taskbar button; also hide via ITaskbarList.
    #[allow(unsafe_code)]
    unsafe {
        SetWindowLongPtrW(aux_hwnd, GWLP_HWNDPARENT, owner_hwnd as isize);
    }

    let _ = aux.with_winit_window(|winit_window| {
        winit_window.set_skip_taskbar(true);
    });
}

#[cfg(windows)]
fn set_window_enabled_windows(window: &slint::Window, enabled: bool) {
    use slint::winit_030::winit::platform::windows::WindowExtWindows;
    use slint::winit_030::WinitWindowAccessor;

    let _ = window.with_winit_window(|winit_window| {
        winit_window.set_enable(enabled);
    });
}

/// 在创建任何窗口之前选定 winit 后端，并关掉默认的窗口透明。
///
/// Slint winit 默认 `WindowAttributes::with_transparent(true)`；Windows 上无框 + DWM
/// 圆角时，顶边会出现一条透到桌面的细缝（自绘标题栏/菜单栏上方）。
/// Slint 只在原生 `MenuBar` 时自动关透明；我们用自绘顶栏，需显式关掉。
/// 见 <https://github.com/slint-ui/slint/issues/7730>。
///
/// 非 Windows 为 no-op（macOS 无框圆角依赖透明，不能一并关掉）。
#[cfg(windows)]
pub fn select_backend() -> AppResult<()> {
    use crate::error::AppError;

    slint::BackendSelector::new()
        .with_winit_window_attributes_hook(|attrs| attrs.with_transparent(false))
        .select()
        .map_err(AppError::from_error)
}

#[cfg(not(windows))]
pub fn select_backend() -> AppResult<()> {
    Ok(())
}

/// 为无框窗口请求 Win11 DWM 圆角。
///
/// `show()` 之后、事件循环首帧之前 HWND 可能尚未就绪；调用方可在
/// `invoke_from_event_loop` 里再试一次。非 Windows / 取不到句柄时为 no-op。
#[cfg(windows)]
pub fn apply_rounded_corners(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };

    let _ = window.with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = win32.hwnd.get() as HWND;
        let preference = DWMWCP_ROUND;
        // DwmSetWindowAttribute 是 Win32 FFI；工作区默认 deny unsafe，此处单独放行。
        #[allow(unsafe_code)]
        let _ = unsafe {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE as u32,
                std::ptr::from_ref(&preference).cast(),
                std::mem::size_of_val(&preference) as u32,
            )
        };
    });
}

#[cfg(not(windows))]
pub fn apply_rounded_corners(_window: &slint::Window) {}

/// 在标题栏按下并拖动时启动原生窗口移动。
///
/// Windows 的惯例是：最大化下从标题栏拖出时先还原，并保持抓取点与光标的相对
/// 位置。这里直接使用 Win32 `WM_NCLBUTTONDOWN`/`HTCAPTION` 交给系统移动循环：
///
/// * 绕开 winit `drag_window()` 的 `window_state.dragging` 守卫。该标记只在
///   `WM_EXITSIZEMOVE` 复位；最大化/还原循环后可能卡住，导致之后所有
///   `drag_window()` 直接 return（WIN-3）。
/// * 最大化时先 `SW_RESTORE`，再按抓取点的相对横向位置把还原窗口摆到光标下，
///   避免窗口还原到旧位置后“跳一下”（WIN-2）。
///
/// 使用同步 `SendMessageW`（标准标题栏拖动写法）：调用在用户松开左键、
/// 移动循环结束后才返回。调用方（`TitleBarDragTouch`）在调用前关掉
/// TouchArea、返回后再打开，避免 PostMessage 异步路径下「下一帧就恢复
/// enabled、左键仍按下 → TouchArea 重新抓住输入 → 拖完要点两次」的问题。
/// 非 Windows / 取不到 HWND 时为 no-op。
#[cfg(windows)]
pub fn start_native_window_move(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, SendMessageW, HTCAPTION, WM_NCLBUTTONDOWN,
    };

    let _ = window.with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = win32.hwnd.get() as HWND;
        // ReleaseCapture/ShowWindow/SendMessageW 等是 Win32 FFI；工作区默认
        // deny unsafe，此处与 apply_rounded_corners 一样单独放行。
        #[allow(unsafe_code)]
        unsafe {
            let mut cursor = POINT { x: 0, y: 0 };
            let cursor = (GetCursorPos(&mut cursor) != 0).then_some((cursor.x, cursor.y));
            restore_maximized_for_drag(hwnd, cursor);
            // ReleaseCapture 需在主线程执行（本回调即在 UI 线程）。
            ReleaseCapture();
            // WM_NCLBUTTONDOWN 的 lParam 是屏幕坐标：低 16 位 x、高 16 位 y。
            let (cursor_x, cursor_y) = cursor.unwrap_or_default();
            let lparam = (((cursor_y as u32) << 16) | (cursor_x as u32 & 0xffff)) as isize;
            // 同步跑完系统移动循环；返回时左键已松开。
            SendMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, lparam);
        }
    });
}

#[cfg(not(windows))]
pub fn start_native_window_move(_window: &slint::Window) {}

/// 最大化窗口开始标题栏拖动前：先还原，再把还原后的窗口按抓取点摆到光标下。
///
/// `cursor` 为屏幕物理坐标；取不到光标时只还原、不重新摆位。
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe fn restore_maximized_for_drag(
    hwnd: windows_sys::Win32::Foundation::HWND,
    cursor: Option<(i32, i32)>,
) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowRect, IsZoomed, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOSIZE,
        SWP_NOZORDER, SW_RESTORE,
    };

    if IsZoomed(hwnd) == 0 {
        return;
    }
    let mut maximized = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut maximized) == 0 {
        return;
    }
    let maximized_width = maximized.right - maximized.left;
    let anchor = cursor.and_then(|(cursor_x, cursor_y)| {
        (maximized_width > 0).then_some((cursor_x, cursor_y, cursor_y - maximized.top))
    });
    // SW_RESTORE 返回调用前的可见状态；最大化窗口必然可见，忽略返回值。
    ShowWindow(hwnd, SW_RESTORE);
    let Some((cursor_x, cursor_y, grab_offset_y)) = anchor else {
        return;
    };
    let mut restored = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut restored) == 0 {
        return;
    }
    let (target_x, target_y) = restored_drag_origin(
        (cursor_x, cursor_y),
        maximized.left,
        maximized_width,
        grab_offset_y,
        restored.right - restored.left,
    );
    SetWindowPos(
        hwnd,
        std::ptr::null_mut(),
        target_x,
        target_y,
        0,
        0,
        SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
    );
}

/// 还原后的窗口左上角（屏幕物理像素），保持抓取点在最大化宽度中的相对横向
/// 位置：`relative_x = (cursor_x - maximized_left) / maximized_width`，再映射到
/// 还原宽度。纵向保留按下点相对窗口顶边的距离 `grab_offset_y`。
#[cfg(any(windows, test))]
fn restored_drag_origin(
    cursor: (i32, i32),
    maximized_left: i32,
    maximized_width: i32,
    grab_offset_y: i32,
    restored_width: i32,
) -> (i32, i32) {
    debug_assert!(maximized_width > 0);
    let relative_x =
        (f64::from(cursor.0 - maximized_left) / f64::from(maximized_width)).clamp(0.0, 1.0);
    let target_x = cursor.0 - (relative_x * f64::from(restored_width)).round() as i32;
    (target_x, cursor.1 - grab_offset_y)
}

#[cfg(test)]
mod tests {
    use super::{centered_aux_origin, restored_drag_origin};

    #[test]
    fn center_aux_origin_centers_in_owner_client_rect() {
        // 1920x1080 客户区里摆 800x600 子窗 → 左上角 (560, 240)。
        assert_eq!(
            centered_aux_origin((0, 0), (1920, 1080), (800, 600)),
            (560, 240)
        );
    }

    #[test]
    fn center_aux_origin_follows_owner_on_secondary_monitor() {
        // owner 客户区从副屏 x=1920 开始：居中只在 owner 自己的客户区内平移。
        assert_eq!(
            centered_aux_origin((1920, 100), (1200, 800), (800, 600)),
            (2120, 200)
        );
    }

    #[test]
    fn center_aux_origin_clamps_oversized_aux_to_top_left() {
        // 子窗比 owner 还大时贴齐左上角，不出现负坐标。
        assert_eq!(
            centered_aux_origin((100, 80), (1200, 800), (1400, 900)),
            (100, 80)
        );
    }

    #[test]
    fn restore_origin_keeps_grab_point_under_cursor() {
        // 1920 宽最大化窗口，光标在中心 → 1440 宽还原窗口中心仍在光标下；
        // 纵向保留按下点距窗口顶边的距离。
        assert_eq!(
            restored_drag_origin((960, 30), 0, 1920, 20, 1440),
            (240, 10)
        );
    }

    #[test]
    fn restore_origin_clamps_relative_x() {
        // 光标越出右缘（极端时序）时按右缘的相对位置摆位，不把窗口甩到光标右侧。
        assert_eq!(
            restored_drag_origin((2200, 40), 0, 1920, 20, 1000),
            (1200, 20)
        );
    }

    #[test]
    fn restore_origin_ignores_window_left_offset() {
        // 最大化窗口从副屏 x=1920 开始：相对位置按窗口自身宽度计算。
        assert_eq!(
            restored_drag_origin((2880, 50), 1920, 1920, 24, 1400),
            (2180, 26)
        );
    }
}
