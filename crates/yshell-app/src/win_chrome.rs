//! Windows 自绘窗口装饰：Win11 圆角、不透明客户区、原生标题栏拖动等与 OS 相关的轻量适配。

use crate::error::AppResult;

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
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::platform::windows::WindowExtWindows;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GWLP_HWNDPARENT, SetWindowLongPtrW,
    };

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
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::platform::windows::WindowExtWindows;

    let _ = window.with_winit_window(|winit_window| {
        winit_window.set_enable(enabled);
    });
}

/// 在创建任何窗口之前选定 winit 后端，并关掉默认的窗口透明。
///
/// Slint winit 默认 `WindowAttributes::with_transparent(true)`。Windows 上无框
/// + DWM 圆角时，顶边会出现一条透到桌面的细缝（自绘标题栏/菜单栏上方）。
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
/// 用 `PostMessageW` 而不是同步 `SendMessageW`，让系统移动循环在回调返回后的
/// 正常消息派发中启动，避免在 Slint/winit 的输入回调里嵌套消息循环。
/// 非 Windows / 取不到 HWND 时为 no-op。
#[cfg(windows)]
pub fn start_native_window_move(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetCursorPos, HTCAPTION, PostMessageW, WM_NCLBUTTONDOWN,
    };

    let _ = window.with_winit_window(|winit_window| {
        let Ok(handle) = winit_window.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return;
        };
        let hwnd = win32.hwnd.get() as HWND;
        // ReleaseCapture/ShowWindow/PostMessageW 等是 Win32 FFI；工作区默认
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
            PostMessageW(hwnd, WM_NCLBUTTONDOWN, HTCAPTION as usize, lparam);
            // 不要在这里 PostMessage WM_LBUTTONUP：系统移动循环还没启动，
            // 队列里的合成释放会立刻结束移动（表现为标题栏拖不动 / 一拖就松）。
            // Slint 抓取残留：TitleBarDragTouch 在上报 drag-requested 前会
            // `enabled = false`（触发 Cancel、丢掉 grab），下一帧再恢复；
            // 仅靠 press-in-bounds 挡误拖，挡不住「点击落到错误控件」。
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
        GetWindowRect, IsZoomed, SW_RESTORE, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        ShowWindow,
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
    use super::restored_drag_origin;

    #[test]
    fn restore_origin_keeps_grab_point_under_cursor() {
        // 1920 宽最大化窗口，光标在中心 → 1440 宽还原窗口中心仍在光标下；
        // 纵向保留按下点距窗口顶边的距离。
        assert_eq!(restored_drag_origin((960, 30), 0, 1920, 20, 1440), (240, 10));
    }

    #[test]
    fn restore_origin_clamps_relative_x() {
        // 光标越出右缘（极端时序）时按右缘的相对位置摆位，不把窗口甩到光标右侧。
        assert_eq!(restored_drag_origin((2200, 40), 0, 1920, 20, 1000), (1200, 20));
    }

    #[test]
    fn restore_origin_ignores_window_left_offset() {
        // 最大化窗口从副屏 x=1920 开始：相对位置按窗口自身宽度计算。
        assert_eq!(restored_drag_origin((2880, 50), 1920, 1920, 24, 1400), (2180, 26));
    }
}

