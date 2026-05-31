//! macOS Dock / Cmd+Tab icon (winit ignores per-window icons on macOS).

use objc2::ClassType;
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{MainThreadMarker, NSData};

pub fn set_from_png(png_bytes: &'static [u8]) {
    let Some(mtm) = MainThreadMarker::new() else {
        eprintln!("difft-file-viewer: application icon requires the main thread");
        return;
    };
    let data = NSData::with_bytes(png_bytes);
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        eprintln!("difft-file-viewer: failed to decode application icon PNG");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    // SAFETY: NSApplication API; image is a valid NSImage from PNG data.
    unsafe { app.setApplicationIconImage(Some(&image)) };
}
