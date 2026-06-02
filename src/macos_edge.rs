//! Use native macOS fullscreen (borderless, separate Space; menu bar hidden until hover).
#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicBool, Ordering};

use i_slint_backend_winit::WinitWindowAccessor;
use winit::window::Window as WinitWindow;

static FILL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Enter native fullscreen on the main display.
pub fn fill_screen(window: &slint::Window) {
    if FILL_IN_PROGRESS.swap(true, Ordering::Acquire) {
        return;
    }
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            FILL_IN_PROGRESS.store(false, Ordering::Release);
        }
    }
    let _guard = Guard;

    window.with_winit_window(|winit_window: &WinitWindow| {
        winit_window.set_maximized(false);
    });
    window.set_fullscreen(true);
}
