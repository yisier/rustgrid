//! The macOS Dock icon.
//!
//! macOS releases ship as a bare executable rather than a `.app` bundle, so AppKit has no
//! `CFBundleIconFile` to read and the Dock — and the ⌘-Tab switcher — fall back to the generic
//! executable icon. GPUI exposes no icon API, so the embedded logo is handed to the shared
//! `NSApplication` directly. That only covers the running process: the icon Finder draws for the
//! file itself can only come from a real bundle (`Info.plist` + `AppIcon.icns`).

use objc2::AnyThread;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

/// The app logo as PNG bytes, padded for the Dock.
///
/// `logo.png` (the asset the 关于 page embeds) fills ~94% of its canvas; the Dock scales whatever
/// it gets to the slot, so a near-full-bleed image renders visibly larger than every other app's
/// icon. `logo_dock.png` is the same art drawn at ~80% of a 512x512 canvas, matching the macOS
/// icon grid's built-in padding (the `.icns` in the release bundle is built from the same file).
static LOGO_PNG: &[u8] = include_bytes!("../assets/logo_dock.png");

/// Hand the embedded logo to the shared `NSApplication` as its icon.
///
/// Must run on the main thread; `main` calls it once from the application's launch callback, where
/// the platform has already created `NSApplication`.
pub(crate) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // SAFETY: `LOGO_PNG` is a program-lifetime static, so the bytes the length describes stay valid
    // while `NSData` copies them.
    let data = unsafe {
        NSData::initWithBytes_length(NSData::alloc(), LOGO_PNG.as_ptr().cast(), LOGO_PNG.len())
    };
    // A malformed PNG yields no image; the generic icon then simply stays.
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    let application = NSApplication::sharedApplication(mtm);
    // SAFETY: `image` is a live `NSImage`, and AppKit retains whatever it keeps.
    unsafe { application.setApplicationIconImage(Some(&image)) };
}
