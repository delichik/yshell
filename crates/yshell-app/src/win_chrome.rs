//! Windows 自绘窗口装饰：Win11 圆角、不透明客户区等与 OS 相关的轻量适配。

use crate::error::AppResult;

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
