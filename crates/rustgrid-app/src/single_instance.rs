//! A single-instance guard, so double-clicking `RustGrid.exe` again just focuses the running app
//! instead of opening a second copy.
//!
//! On Windows this is a named kernel mutex (the OS releases it automatically when the process
//! exits). Elsewhere it is an advisory `flock` on a file under the temp dir, which is likewise
//! released by the OS on exit — so a crash never leaves a stale lock. The returned guard must be
//! kept alive for as long as the app runs.
//!
//! A later launch may carry file arguments (a SQLite database dragged onto the executable). It
//! cannot show them itself, so [`forward_to_running`] leaves the paths in a small temp file that
//! the running instance polls through [`take_forwarded_paths`].

use std::path::PathBuf;

/// The single-instance lock. Keep the value alive in `main`; dropping it releases the lock.
pub struct Guard {
    _inner: imp::Inner,
}

/// Try to become the single running instance. Returns `None` when another instance already holds
/// the lock, in which case the caller should exit immediately.
pub fn acquire() -> Option<Guard> {
    imp::acquire().map(|_inner| Guard { _inner })
}

/// The file a second instance uses to hand paths to the running one.
fn pending_paths_file() -> PathBuf {
    std::env::temp_dir().join("rustgrid.pending-open")
}

/// Hand `paths` to the running instance. A no-op when there is nothing to open.
pub fn forward_to_running(paths: &[PathBuf]) {
    if paths.is_empty() {
        return;
    }
    let body = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    let target = pending_paths_file();
    let temporary = target.with_extension(format!("tmp-{}", std::process::id()));
    if std::fs::write(&temporary, body).is_err() {
        return;
    }
    // `rename` replaces an existing file atomically on Unix; Windows refuses to overwrite, so the
    // old request is cleared first. A running instance that reads in that instant simply sees no
    // request and picks this one up on its next poll.
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::rename(&temporary, &target);
}

/// Paths another launch asked this instance to open. Reading consumes the request.
pub fn take_forwarded_paths() -> Vec<PathBuf> {
    let target = pending_paths_file();
    let Ok(body) = std::fs::read_to_string(&target) else {
        return Vec::new();
    };
    let _ = std::fs::remove_file(&target);
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

#[cfg(target_os = "windows")]
mod imp {
    use std::time::Duration;

    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::w;

    pub(super) struct Inner;

    pub(super) fn acquire() -> Option<Inner> {
        // A process relaunched by the updater retries briefly: the instance it replaced may not have
        // exited and released the mutex yet.
        let attempts = attempts();
        for _ in 0..attempts {
            // A global name; on success the handle is deliberately never closed, so the mutex lives
            // for the whole process (the OS releases it on exit).
            unsafe {
                match CreateMutexW(None, false, w!("RustGrid.SingleInstance")) {
                    Ok(handle) => {
                        if GetLastError() != ERROR_ALREADY_EXISTS {
                            return Some(Inner);
                        }
                        // Another instance owns the mutex: drop our reference and wait for it.
                        let _ = CloseHandle(handle);
                    }
                    // If the mutex cannot be created for any reason, fail open and let the app run.
                    Err(_) => return Some(Inner),
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }

    fn attempts() -> u32 {
        if std::env::var_os("RUSTGRID_RESTARTED").is_some() {
            100
        } else {
            1
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    pub(super) struct Inner {
        _file: Option<File>,
    }

    pub(super) fn acquire() -> Option<Inner> {
        let path = std::env::temp_dir().join("rustgrid.single-instance.lock");
        // A process relaunched by the updater retries briefly: the instance it replaced may not have
        // exited and released the lock yet.
        let attempts = if std::env::var_os("RUSTGRID_RESTARTED").is_some() {
            100
        } else {
            1
        };
        for _ in 0..attempts {
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
            let locked =
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
            if locked {
                return Some(Inner { _file: Some(file) });
            }
            drop(file);
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}
