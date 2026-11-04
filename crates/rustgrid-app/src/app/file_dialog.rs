//! The native *open* file picker every feature goes through.
//!
//! macOS has no filename-extension matching for `NSOpenPanel`: each extension handed to `rfd` is
//! resolved through LaunchServices into a uniform type identifier, and an extension no installed
//! app declares (`.db`, `.sqlite`, `.sql`, `.rgc`, `.ncx`, ...) has no identifier to match. The
//! panel runs out of process, so it then enables no file at all — every SQLite database, script
//! file and connection bundle came up greyed out and unselectable (issue #36). Windows and Linux
//! compare the extension literally, so the filter keeps its value there and stays.
//!
//! Save dialogs do not go through here: their filter only appends the extension to the name the
//! user typed, so it never makes an existing file unselectable.

use rfd::AsyncFileDialog;

/// An open dialog titled `title` offering `extensions` as its only file type.
///
/// On macOS the filter is dropped (see the module comment), so any file can be picked and the
/// caller's own validation reports one that is not what it expected; on Windows and Linux the
/// dialog keeps the extension filter.
pub(super) fn open_dialog(
    title: impl Into<String>,
    filter_name: &str,
    extensions: &[&str],
) -> AsyncFileDialog {
    let dialog = AsyncFileDialog::new().set_title(title);
    #[cfg(not(target_os = "macos"))]
    let dialog = dialog.add_filter(filter_name, extensions);
    #[cfg(target_os = "macos")]
    let _ = (filter_name, extensions);
    dialog
}
