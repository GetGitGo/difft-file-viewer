//! Snap a borderless window flush to the monitor work area on Windows.
#![cfg(target_os = "windows")]

use i_slint_backend_winit::{EventResult, WinitWindowAccessor};
use std::sync::atomic::{AtomicBool, Ordering};
use winit::event::WindowEvent;
use winit::platform::windows::{CornerPreference, MonitorHandleExtWindows, WindowExtWindows};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window as WinitWindow;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMWA_ALLOW_NCPAINT,
    DWMWA_NCRENDERING_POLICY,
};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE,
    GWLP_WNDPROC, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WM_ACTIVATE, WM_DWMCOMPOSITIONCHANGED, WM_NCACTIVATE, WNDPROC, WS_BORDER, WS_CAPTION,
    WS_EX_CLIENTEDGE, WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
};

/// Left/top bleed clips UI; bottom bleed hides the last list row when scrolled to the end.
const DWM_INSET_LEFT: i32 = 0;
const DWM_INSET_TOP: i32 = 0;
const DWM_INSET_RIGHT: i32 = 8;
const DWM_INSET_BOTTOM: i32 = 0;

/// Undocumented messages that draw themed caption/frame over the client area on focus.
const WM_NCUAHDRAWCAPTION: u32 = 0x00AE;
const WM_NCUAHDRAWFRAME: u32 = 0x00AF;

static SUBCLASS_INSTALLED: AtomicBool = AtomicBool::new(false);
static ORIGINAL_WNDPROC: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

fn monitor_work_area(hmonitor: HMONITOR) -> windows::Win32::Foundation::RECT {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        let _ = GetMonitorInfoW(hmonitor, &mut info);
    }
    info.rcWork
}

unsafe fn hwnd_from_winit(winit_window: &WinitWindow) -> Option<HWND> {
    let handle = winit_window.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(win32) => Some(HWND(win32.hwnd.get() as *mut _)),
        _ => None,
    }
}

unsafe fn strip_frame_borders(hwnd: HWND) {
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    let style_mask = (WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX
        | WS_BORDER)
        .0 as isize;
    SetWindowLongPtrW(hwnd, GWL_STYLE, style & !style_mask);
    let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    let ex_style_mask = (WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE).0 as isize;
    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style & !ex_style_mask);
}

unsafe fn disable_dwm_nc_rendering(hwnd: HWND) {
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyTopHeight: 0,
        cyBottomHeight: 0,
    };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    let policy = DWMNCRP_DISABLED.0 as u32;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_NCRENDERING_POLICY,
        &policy as *const _ as *const _,
        std::mem::size_of::<u32>() as u32,
    );
    let allow_nc_paint = 0u32;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_ALLOW_NCPAINT,
        &allow_nc_paint as *const _ as *const _,
        std::mem::size_of::<u32>() as u32,
    );
}

unsafe fn refresh_frame(hwnd: HWND) {
    let _ = SetWindowPos(
        hwnd,
        None,
        0,
        0,
        0,
        0,
        SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
    );
}

unsafe fn apply_borderless_style(hwnd: HWND) {
    strip_frame_borders(hwnd);
    disable_dwm_nc_rendering(hwnd);
    refresh_frame(hwnd);
}

unsafe extern "system" fn borderless_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCACTIVATE => return LRESULT(1),
        WM_NCUAHDRAWCAPTION | WM_NCUAHDRAWFRAME => return LRESULT(0),
        WM_ACTIVATE | WM_DWMCOMPOSITIONCHANGED => {
            apply_borderless_style(hwnd);
        }
        _ => {}
    }
    let original = ORIGINAL_WNDPROC.load(Ordering::SeqCst);
    if original == 0 {
        return LRESULT(0);
    }
    CallWindowProcW(
        std::mem::transmute::<isize, WNDPROC>(original),
        hwnd,
        msg,
        wparam,
        lparam,
    )
}

unsafe fn install_borderless_subclass(hwnd: HWND) {
    if SUBCLASS_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let original = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
    ORIGINAL_WNDPROC.store(original, Ordering::SeqCst);
    SetWindowLongPtrW(hwnd, GWLP_WNDPROC, borderless_wnd_proc as isize);
}

unsafe fn with_hwnd(window: &slint::Window, f: impl FnOnce(HWND)) {
    window.with_winit_window(|winit_window: &WinitWindow| {
        if let Some(hwnd) = unsafe { hwnd_from_winit(winit_window) } {
            f(hwnd);
        }
    });
}

/// Re-apply borderless styles (e.g. after Alt-Tab focus).
pub fn reinforce_borderless(window: &slint::Window) {
    unsafe {
        with_hwnd(window, |hwnd| apply_borderless_style(hwnd));
    }
}

/// Block DWM/native caption painting that appears on window activation.
pub fn install_borderless_hooks(window: &slint::Window) {
    unsafe {
        with_hwnd(window, |hwnd| install_borderless_subclass(hwnd));
    }

    window.on_winit_window_event(|window, event| {
        if matches!(event, WindowEvent::Focused(true)) {
            reinforce_borderless(window);
        }
        EventResult::Propagate
    });
}

/// Position/size the native window flush to the monitor work area (physical pixels).
pub fn fill_work_area(window: &slint::Window) {
    window.with_winit_window(|winit_window: &WinitWindow| {
        winit_window.set_undecorated_shadow(false);
        winit_window.set_corner_preference(CornerPreference::DoNotRound);

        let Some(monitor) = winit_window.current_monitor() else {
            return;
        };
        let work = monitor_work_area(HMONITOR(monitor.hmonitor() as *mut core::ffi::c_void));
        let width = (work.right - work.left).max(0);
        let height = (work.bottom - work.top).max(0);

        window.set_maximized(false);

        let Some(hwnd) = (unsafe { hwnd_from_winit(winit_window) }) else {
            return;
        };
        unsafe {
            install_borderless_subclass(hwnd);
            apply_borderless_style(hwnd);
            // Bleed into DWM inset; skip top so the header row is not clipped off-screen.
            let _ = SetWindowPos(
                hwnd,
                None,
                work.left - DWM_INSET_LEFT,
                work.top - DWM_INSET_TOP,
                width + DWM_INSET_LEFT + DWM_INSET_RIGHT,
                height + DWM_INSET_TOP + DWM_INSET_BOTTOM,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    });
}
