//! Opening files by dragging them onto the window (or launching RustGrid with one as its argument).
//!
//! The drop frame itself is deliberately generic: it says only that the file will be opened, so a
//! future format plugs into [`classify_dropped_file`] instead of needing its own overlay. SQLite is
//! the format implemented today: a dropped database file becomes an ordinary connection named after
//! the file, added to the tree and opened. A file that already has a connection is not added twice:
//! its connection is selected and opened again. While a file is dragged over the window, a scrim
//! shows where it will land; the overlay is one element with a short opacity animation, so the
//! indicator costs nothing per mouse move.

use std::path::{Path, PathBuf};

use gpui::ExternalPaths;
use rustgrid_core::{ConnectionOptions, ConnectionProfile, DriverId};

use super::*;

/// The extensions a database file may carry. SQLite accepts any name, so these are only a hint for
/// the file picker's filter and for an empty file whose header has not been written yet; the header
/// check below is authoritative.
pub(super) const SQLITE_EXTENSIONS: &[&str] = &["db", "db3", "s3db", "sl3", "sqlite", "sqlite3"];

/// The 16-byte header every SQLite database file starts with.
const SQLITE_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// What a dropped file should be opened as. This is the extension point for new drag-and-drop
/// formats: add a variant, classify it below, and handle it in [`AppView::open_dropped_file`].
enum DroppedFile {
    SqliteDatabase(PathBuf),
}

/// Whether `path` carries one of SQLite's conventional extensions. The header is authoritative for
/// a normal database, but an encrypted SQLite file (SQLCipher) or a database still being copied has
/// no readable header at drop time; the extension still says what the user meant to open, and the
/// connection attempt reports a useful error instead of "unsupported".
fn has_sqlite_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            SQLITE_EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
        })
}

/// Decide what a dropped file is. Unrecognised formats return `None`; the drop handler then shows
/// the generic "unsupported file type" message.
fn classify_dropped_file(path: &Path) -> Option<DroppedFile> {
    if is_sqlite_file(path) || has_sqlite_extension(path) {
        return Some(DroppedFile::SqliteDatabase(path.to_path_buf()));
    }
    None
}

/// Whether `path` is a SQLite database file: either its header proves it, or it is an empty file
/// with one of the conventional extensions (SQLite writes the header when the first page is
/// committed).
fn is_sqlite_file(path: &Path) -> bool {
    let Some(meta) = std::fs::metadata(path).ok() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    let mut header = [0u8; SQLITE_HEADER.len()];
    let header_ok = std::fs::File::open(path)
        .and_then(|mut file| {
            use std::io::Read as _;
            file.read_exact(&mut header)
        })
        .is_ok()
        && &header == SQLITE_HEADER;
    header_ok || (meta.len() == 0 && has_sqlite_extension(path))
}

/// The dropped files this build can open, in the order they were dropped.
fn dropped_files(paths: &ExternalPaths) -> Vec<DroppedFile> {
    paths
        .paths()
        .iter()
        .filter_map(|path| classify_dropped_file(path))
        .collect()
}

/// Paths compare case-insensitively on Windows, which is where dragging a file onto the executable
/// is most likely.
fn same_file(left: &str, right: &Path) -> bool {
    let right = right.to_string_lossy();
    if cfg!(windows) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

impl AppView {
    /// The index of an existing connection that already points at `path`.
    fn sqlite_connection_index(&self, path: &Path) -> Option<usize> {
        let canonical = std::fs::canonicalize(path).ok();
        self.connections.iter().position(|node| {
            if node.profile.driver.as_str() != "sqlite" {
                return false;
            }
            let Some(database) = node.profile.database.as_deref() else {
                return false;
            };
            if same_file(database, path) {
                return true;
            }
            canonical.as_ref().is_some_and(|canonical| {
                std::fs::canonicalize(database)
                    .ok()
                    .is_some_and(|existing| existing == *canonical)
            })
        })
    }

    /// A connection name unique among the configured ones: the file stem, then `name (2)`.
    fn unique_sqlite_name(&self, path: &Path) -> String {
        let base = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.trim().is_empty())
            .map(str::to_string)
            .or_else(|| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "SQLite".to_string());
        let used: std::collections::HashSet<&str> = self
            .connections
            .iter()
            .map(|node| node.profile.name.as_str())
            .collect();
        if !used.contains(base.as_str()) {
            return base;
        }
        let mut suffix = 2;
        loop {
            let candidate = format!("{base} ({suffix})");
            if !used.contains(candidate.as_str()) {
                return candidate;
            }
            suffix += 1;
        }
    }

    /// Open one classified dropped file. A new format adds its arm here and its classifier above.
    fn open_dropped_file(&mut self, file: DroppedFile, cx: &mut Context<'_, Self>) {
        match file {
            DroppedFile::SqliteDatabase(path) => self.open_sqlite_file(path, cx),
        }
    }

    /// Open a dropped (or command-line) SQLite file: reuse its connection when the file is already
    /// configured, otherwise add and connect a new one named after the file. Command-line paths
    /// have not been classified, so they are checked here; a dropped file already passed
    /// [`classify_dropped_file`] and must not be rejected again by the header check.
    pub(crate) fn open_sqlite_path(&mut self, path: PathBuf, cx: &mut Context<'_, Self>) {
        if !is_sqlite_file(&path) {
            self.toast(
                ToastKind::Error,
                t!("file_drop.unsupported").to_string(),
                cx,
            );
            return;
        }
        self.open_sqlite_file(path, cx);
    }

    /// Open a file already known to be a SQLite database.
    fn open_sqlite_file(&mut self, path: PathBuf, cx: &mut Context<'_, Self>) {
        if let Some(index) = self.sqlite_connection_index(&path) {
            self.focus_sqlite_connection(index, cx);
            return;
        }

        let profile = ConnectionProfile {
            id: format!("conn-{}", crate::form::now_nanos()),
            name: self.unique_sqlite_name(&path),
            driver: DriverId::new("sqlite"),
            host: String::new(),
            port: 0,
            username: String::new(),
            database: Some(path.to_string_lossy().into_owned()),
            options: BTreeMap::new(),
            settings: ConnectionOptions::default(),
        };
        self.connect_sqlite_profile(profile, cx);
    }

    /// Select a connection and make sure it is open, then show its first database's objects.
    fn focus_sqlite_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.tree_selection.select_one(format!("conn-{index}"));
        let Some((profile_id, connected, connecting, loaded)) =
            self.connections.get_mut(index).map(|node| {
                node.expanded = true;
                (
                    node.profile.id.clone(),
                    matches!(&node.status, ConnectionStatus::Connected(_)),
                    matches!(&node.status, ConnectionStatus::Connecting),
                    matches!(&node.databases, Loadable::Loaded(_)),
                )
            })
        else {
            return;
        };
        if connected {
            if loaded {
                self.open_first_database(index, cx);
            } else {
                self.pending_open_databases.insert(profile_id);
                self.load_databases(index, cx);
            }
        } else {
            self.pending_open_databases.insert(profile_id);
            if !connecting {
                self.connect(index, cx);
            }
        }
        cx.notify();
    }

    /// Show the first database of `index` (SQLite has exactly one, `main`), opening it when it is
    /// still closed and merely re-selecting it when a restored session already opened it.
    pub(super) fn open_first_database(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(profile_id) = self
            .connections
            .get(index)
            .map(|node| node.profile.id.clone())
        {
            self.pending_open_databases.remove(&profile_id);
        }
        let Some(has_database) =
            self.connections
                .get(index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => Some(!databases.is_empty()),
                    _ => None,
                })
        else {
            return;
        };
        if !has_database {
            return;
        }
        let opened = matches!(
            self.connections
                .get(index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => databases.first(),
                    _ => None,
                }),
            Some(database) if database.opened
        );
        if opened {
            self.show_database_objects(index, 0, cx);
        } else {
            self.open_database(index, 0, cx);
        }
    }

    /// Connect a freshly built SQLite profile, add it to the tree and open its database once the
    /// connection is up.
    fn connect_sqlite_profile(&mut self, profile: ConnectionProfile, cx: &mut Context<'_, Self>) {
        let Some(driver) = self.registry.get(&profile.driver) else {
            self.error_dialog = Some(t!("error.driver_missing").to_string());
            cx.notify();
            return;
        };
        let config = ConnectionConfig {
            driver: profile.driver.clone(),
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            password: None,
            database: profile.database.clone(),
            options: profile.options.clone(),
            settings: profile.settings.clone(),
        };
        let runtime = self.runtime.clone();
        let profile_id = profile.id.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };
            let _ = this.update(cx, |view, cx| match result {
                Ok(connection) => {
                    view.connections.push(ConnectionNode {
                        profile,
                        password: None,
                        password_saved: false,
                        status: ConnectionStatus::Connected(Arc::from(connection)),
                        databases: Loadable::Idle,
                        expanded: true,
                    });
                    let index = view.connections.len() - 1;
                    view.add_connection_to_tree(profile_id.clone(), None);
                    view.persist_profiles(cx);
                    view.persist_settings();
                    view.tree_selection.select_one(format!("conn-{index}"));
                    view.pending_open_databases.insert(profile_id);
                    view.load_databases(index, cx);
                    view.toast(
                        ToastKind::Info,
                        t!(
                            "file_drop.opened",
                            name = view.connections[index].profile.name
                        )
                        .to_string(),
                        cx,
                    );
                }
                Err(error) => {
                    view.error_dialog = Some(error.to_string());
                }
            });
        })
        .detach();
    }

    /// The file drag crossed onto the window: show the drop scrim. It is shown for any dragged
    /// file, because the frame is generic; the classifier decides on release whether this build
    /// knows how to open it. Only a change repaints (a drag move fires constantly).
    pub(super) fn file_drop_entered(&mut self, paths: &ExternalPaths, cx: &mut Context<'_, Self>) {
        if self.file_drop_checked {
            return;
        }
        self.file_drop_checked = true;
        let active = !paths.paths().is_empty();
        if self.file_drop_active != active {
            self.file_drop_active = active;
            cx.notify();
        }
    }

    /// The drag left the window (or the platform cancelled it): take the scrim down. This is
    /// called from both the window root and the overlay itself, because the overlay sits on top of
    /// the root's hitbox and would otherwise be the element the exit event is tested against.
    pub(super) fn file_drop_exited(&mut self, cx: &mut Context<'_, Self>) {
        self.file_drop_checked = false;
        if self.file_drop_active {
            self.file_drop_active = false;
            cx.notify();
        }
    }

    /// The files were dropped: open every one this build recognises, or say why none was opened.
    pub(super) fn file_drop_submitted(
        &mut self,
        paths: &ExternalPaths,
        cx: &mut Context<'_, Self>,
    ) {
        self.file_drop_exited(cx);
        let files = dropped_files(paths);
        if files.is_empty() {
            self.toast(
                ToastKind::Error,
                t!("file_drop.unsupported").to_string(),
                cx,
            );
            return;
        }
        for file in files {
            self.open_dropped_file(file, cx);
        }
    }

    /// The drag indicator: the window dims and a dashed drop target names the action. The frame is
    /// format-agnostic on purpose — a new droppable format only adds a classifier.
    pub(super) fn render_file_drop_overlay(
        &self,
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        div()
            .id("file-drop-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            // A plain alpha scrim rather than a backdrop filter: it reads as the interface
            // receding, costs one quad, and works on every platform GPUI supports. It is
            // deliberately not `.occlude()`: the root still needs to stay hovered so its drop
            // listener receives the release. The overlay carries the exit/up handlers as well,
            // because it is the topmost hitbox while it is shown; without them a cancelled drag
            // left the frame on screen.
            .bg(rgba((theme.window_bg << 8) | 0xC8))
            .on_file_drop_exit(
                cx.listener(|this, _event: &gpui::FileDropEvent, _window, cx| {
                    this.file_drop_exited(cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.file_drop_exited(cx);
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                // A cancelled platform drag may not deliver an exit event; the next ordinary
                // pointer move (no button held) then takes the frame down.
                if event.pressed_button.is_none() {
                    this.file_drop_exited(cx);
                }
            }))
            .with_animation(
                "file-drop-overlay-fade",
                Animation::new(Duration::from_millis(120)),
                |overlay, delta| overlay.opacity(delta),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_3()
                    .w(px(420.0))
                    .px_8()
                    .py_10()
                    .rounded(px(14.0))
                    .border_2()
                    .border_color(rgb(theme.brand))
                    .bg(rgb(theme.dialog_bg))
                    .child(
                        svg()
                            .path("icons/file.svg")
                            .w(px(40.0))
                            .h(px(40.0))
                            .flex_none()
                            .text_color(rgb(theme.brand)),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(theme.text))
                            .child(t!("file_drop.title").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.text_muted))
                            .child(t!("file_drop.hint").to_string()),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sqlite_header_is_recognised() {
        let dir =
            std::env::temp_dir().join(format!("rustgrid-sqlite-drop-{}", crate::form::now_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.data");
        std::fs::write(&file, SQLITE_HEADER).unwrap();
        assert!(is_sqlite_file(&file));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_non_empty_file_with_a_sqlite_extension_is_still_offered_to_sqlite() {
        // An encrypted (SQLCipher) database or a file still being copied has no readable header;
        // its extension still says the user meant a SQLite database, so the connect attempt must
        // report the real error instead of "unsupported file type".
        let dir =
            std::env::temp_dir().join(format!("rustgrid-sqlite-drop-{}", crate::form::now_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("encrypted.sqlite");
        std::fs::write(&file, b"not a plain sqlite header").unwrap();
        assert!(matches!(
            classify_dropped_file(&file),
            Some(DroppedFile::SqliteDatabase(path)) if path == file
        ));
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"not a database").unwrap();
        assert!(classify_dropped_file(&other).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_database_file_keeps_its_conventional_extension() {
        let dir =
            std::env::temp_dir().join(format!("rustgrid-sqlite-drop-{}", crate::form::now_nanos()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.db");
        std::fs::write(&file, b"").unwrap();
        assert!(is_sqlite_file(&file));
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"not a database").unwrap();
        assert!(!is_sqlite_file(&other));
        std::fs::remove_dir_all(&dir).ok();
    }
}
