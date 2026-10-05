//! A single-instance guard, so double-clicking `RustGrid.exe` again just focuses the running app
//! instead of opening a second copy.
//!
//! On Windows this is a named kernel mutex (the OS releases it automatically when the process
//! exits). Elsewhere it is an advisory `flock` on a file under the temp dir, which is likewise
//! released by the OS on exit — so a crash never leaves a stale lock. The returned guard must be
//! kept alive for as long as the app runs.

/// The single-instance lock. Keep the value alive in `main`; dropping it releases the lock.
pub struct Guard {
    _inner: imp::Inner,
}

/// Try to become the single running instance. Returns `None` when another instance already holds
/// the lock, in which case the caller should exit immediately.
pub fn acquire() -> Option<Guard> {
    imp::acquire().map(|_inner| Guard { _inner })
}

#[cfg(target_os = "windows")]
mod imp {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    pub(super) struct Inner;

    pub(super) fn acquire() -> Option<Inner> {
        // A global name; the handle is deliberately never closed, so the mutex lives for the whole
        // process (the OS releases it on exit).
        unsafe {
            match CreateMutexW(None, false, w!("RustGrid.SingleInstance")) {
                Ok(_handle) => (GetLastError() != ERROR_ALREADY_EXISTS).then_some(Inner),
                // If the mutex cannot be created for any reason, fail open and let the app run.
                Err(_) => Some(Inner),
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;

    pub(super) struct Inner {
        _file: Option<File>,
    }

    pub(super) fn acquire() -> Option<Inner> {
        let path = std::env::temp_dir().join("rustgrid.single-instance.lock");
        let file = match OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
        {
            Ok(file) => file,
            // Cannot open the lock file: fail open rather than locking the user out.
            Err(_) => return Some(Inner { _file: None }),
        };
        // A non-blocking exclusive lock; the OS drops it when the process exits.
        let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        locked.then_some(Inner { _file: Some(file) })
    }
}
