//! The app's keymap: every rebindable command, its key context and its built-in keystrokes.
//!
//! The Options window's 快捷键 page lists [`COMMANDS`], and the user's overrides (command id ->
//! keystrokes, stored in `settings.json`) replace a command's defaults. [`bind_keymap`] installs the
//! result. gpui's keymap has no removal API, so a re-bind first shadows every keystroke the previous
//! call bound with a `NoAction` in the same context; a later binding outranks an earlier one for
//! the same keystroke, so only the app's stale bindings stop firing and the kit's own keep working.
//!
//! Mechanics that must not move (Tab / Up / Down inside a cell editor, Tab accepting a completion)
//! are bound in `AppView::new` and are not commands.

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{
    Action, App, KeyBinding, KeyBindingContextPredicate, Keystroke, NoAction, OperatingSystem,
};
use gpui_kit::component::input::ShowCompletions;

use super::{
    BACKUP_LIST_CONTEXT, BackupFocusSearch, CloseActiveTab, CloseAllTabs, CloseOtherTabs,
    CopyBackupFile, CopyQueryFile, CopyStatusSql, DESIGNER_CONTEXT, DeleteTransferConfig,
    DesignerCopy, DesignerPaste, DesignerSelectAll, DuplicateLines, GRID_CONTEXT,
    GRID_STATUS_CONTEXT, GridDuplicateRow, GridEditValue, GridFirstCell, GridLastCell,
    GridNextCell, GridPageDown, GridPageUp, GridPrevCell, GridRefresh, GridRowEnd, GridRowStart,
    GridSelectPageDown, GridSelectPageUp, MoveLinesDown, MoveLinesUp, NewQueryTab, NextTab,
    OBJECT_LIST_CONTEXT, ObjectDown, ObjectEnd, ObjectFocusSearch, ObjectHome, ObjectLeft,
    ObjectOpen, ObjectPageDown, ObjectPageUp, ObjectRename, ObjectRight, ObjectSelectAll, ObjectUp,
    OpenTransferConfig, PAGE_CONTEXT, PasteBackupFile, PasteQueryFile, PrevTab,
    QUERY_EDITOR_CONTEXT, QUERY_LIST_CONTEXT, QueryFocusSearch, ROW_FORM_CONTEXT, RefreshPage,
    RenameBackupFile, RenameQueryFile, RunQuery, RunSelectedQuery, SaveDesign, SaveQuery,
    SaveRowForm, SelectAllBackups, SelectAllOccurrences, SelectAllQueries, SelectAllStatusSql,
    SelectAllTransfers, SwapLineUp, TRANSFER_LIST_CONTEXT, TREE_CONTEXT, ToggleComment,
    TreeCollapse, TreeDown, TreeEnd, TreeExpand, TreeFocusSearch, TreeHome, TreeOpen, TreePageDown,
    TreePageUp, TreeRename, TreeSelectAll, TreeUp, USER_LIST_CONTEXT, UserFocusSearch,
    WORKSPACE_CONTEXT,
};

/// Where a command lives, for grouping the 快捷键 page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommandGroup {
    General,
    ConnectionTree,
    ObjectList,
    TableGrid,
    QueryEditor,
    Designer,
    Lists,
}

impl CommandGroup {
    pub(super) const ALL: [CommandGroup; 7] = [
        CommandGroup::General,
        CommandGroup::ConnectionTree,
        CommandGroup::ObjectList,
        CommandGroup::TableGrid,
        CommandGroup::QueryEditor,
        CommandGroup::Designer,
        CommandGroup::Lists,
    ];

    /// The i18n key of the group's heading.
    pub(super) fn label_key(self) -> &'static str {
        match self {
            CommandGroup::General => "keymap.group.general",
            CommandGroup::TableGrid => "keymap.group.grid",
            CommandGroup::ConnectionTree => "keymap.group.tree",
            CommandGroup::ObjectList => "keymap.group.objects",
            CommandGroup::QueryEditor => "keymap.group.query_editor",
            CommandGroup::Designer => "keymap.group.designer",
            CommandGroup::Lists => "keymap.group.lists",
        }
    }
}

/// One rebindable command.
pub(super) struct Command {
    /// The stable id stored in `settings.json` (`"query.run"`).
    pub id: &'static str,
    /// The i18n key of its label on the 快捷键 page.
    pub label_key: &'static str,
    pub group: CommandGroup,
    /// The gpui key context the binding is registered in.
    pub context: &'static str,
    /// Built-in keystrokes (gpui syntax; `secondary` is Ctrl off macOS and ⌘ on it).
    pub defaults: &'static [&'static str],
    /// Built-in keystrokes on macOS, where they differ from `defaults`.
    pub mac_defaults: Option<&'static [&'static str]>,
    /// Builds `keystroke`'s binding in `context`; `None` when it does not parse.
    pub bind: fn(&str, Option<&'static str>, &App) -> Option<KeyBinding>,
}

impl Command {
    /// This platform's built-in keystrokes.
    pub(super) fn default_keys(&self) -> &'static [&'static str] {
        match self.mac_defaults {
            Some(keys) if cfg!(target_os = "macos") => keys,
            _ => self.defaults,
        }
    }
}

/// Build a binding from a runtime keystroke. `settings.json` can rebind anything, so a command's
/// keystrokes are never `'static`; `KeyBinding::new` (static, `DummyKeyboardMapper`) therefore
/// cannot be used. `KeyBinding::load` parses against the host OS (so `secondary-r` is ⌘R on macOS
/// and Ctrl+R elsewhere) and resolves the context with the app's own keyboard mapper.
fn load_binding(
    keystroke: &str,
    action: impl Action,
    context: Option<&'static str>,
    cx: &App,
) -> Option<KeyBinding> {
    let context = context.map(|context| {
        KeyBindingContextPredicate::parse(context)
            .map(Rc::new)
            .expect("the app's key contexts are valid predicates")
    });
    KeyBinding::load(keystroke, Box::new(action), context, false, None, cx).ok()
}

macro_rules! bind {
    ($action:expr) => {
        |keystroke, context, cx| load_binding(keystroke, $action, context, cx)
    };
}

/// A navigation command of `group` (key `context`) bound to `keys` on every platform.
macro_rules! nav_command {
    ($group:expr, $context:expr, $id:literal, $label:literal, $keys:expr, $action:expr) => {
        Command {
            id: $id,
            label_key: $label,
            group: $group,
            context: $context,
            defaults: $keys,
            mac_defaults: None,
            bind: bind!($action),
        }
    };
}

/// A connection-tree navigation command.
macro_rules! tree_command {
    ($id:literal, $label:literal, $keys:expr, $action:expr) => {
        nav_command!(
            CommandGroup::ConnectionTree,
            TREE_CONTEXT,
            $id,
            $label,
            $keys,
            $action
        )
    };
}

/// An object-list navigation command.
macro_rules! object_command {
    ($id:literal, $label:literal, $keys:expr, $action:expr) => {
        nav_command!(
            CommandGroup::ObjectList,
            OBJECT_LIST_CONTEXT,
            $id,
            $label,
            $keys,
            $action
        )
    };
}

/// A table-grid command.
macro_rules! grid_command {
    ($id:literal, $label:literal, $keys:expr, $action:expr) => {
        nav_command!(
            CommandGroup::TableGrid,
            GRID_CONTEXT,
            $id,
            $label,
            $keys,
            $action
        )
    };
}

/// A query-editor command.
macro_rules! editor_command {
    ($id:literal, $label:literal, $keys:expr, $action:expr) => {
        nav_command!(
            CommandGroup::QueryEditor,
            QUERY_EDITOR_CONTEXT,
            $id,
            $label,
            $keys,
            $action
        )
    };
}

/// Every rebindable command, in 快捷键-page order within each group.
pub(super) const COMMANDS: &[Command] = &[
    tree_command!("tree.up", "keymap.tree.up", &["up"], TreeUp),
    tree_command!("tree.down", "keymap.tree.down", &["down"], TreeDown),
    tree_command!(
        "tree.page_up",
        "keymap.tree.page_up",
        &["pageup"],
        TreePageUp
    ),
    tree_command!(
        "tree.page_down",
        "keymap.tree.page_down",
        &["pagedown"],
        TreePageDown
    ),
    tree_command!("tree.home", "keymap.tree.home", &["home"], TreeHome),
    tree_command!("tree.end", "keymap.tree.end", &["end"], TreeEnd),
    tree_command!("tree.expand", "keymap.tree.expand", &["right"], TreeExpand),
    tree_command!(
        "tree.collapse",
        "keymap.tree.collapse",
        &["left"],
        TreeCollapse
    ),
    tree_command!("tree.open", "keymap.tree.open", &["enter"], TreeOpen),
    tree_command!("tree.rename", "keymap.tree.rename", &["f2"], TreeRename),
    tree_command!(
        "tree.select_all",
        "keymap.tree.select_all",
        &["secondary-a"],
        TreeSelectAll
    ),
    tree_command!(
        "tree.focus_search",
        "keymap.tree.focus_search",
        &["secondary-f"],
        TreeFocusSearch
    ),
    object_command!("objects.up", "keymap.objects.up", &["up"], ObjectUp),
    object_command!("objects.down", "keymap.objects.down", &["down"], ObjectDown),
    object_command!("objects.left", "keymap.objects.left", &["left"], ObjectLeft),
    object_command!(
        "objects.right",
        "keymap.objects.right",
        &["right"],
        ObjectRight
    ),
    object_command!(
        "objects.page_up",
        "keymap.objects.page_up",
        &["pageup"],
        ObjectPageUp
    ),
    object_command!(
        "objects.page_down",
        "keymap.objects.page_down",
        &["pagedown"],
        ObjectPageDown
    ),
    object_command!("objects.home", "keymap.objects.home", &["home"], ObjectHome),
    object_command!("objects.end", "keymap.objects.end", &["end"], ObjectEnd),
    object_command!(
        "objects.open",
        "keymap.objects.open",
        &["enter"],
        ObjectOpen
    ),
    object_command!(
        "objects.rename",
        "keymap.objects.rename",
        &["f2"],
        ObjectRename
    ),
    object_command!(
        "objects.select_all",
        "keymap.objects.select_all",
        &["secondary-a"],
        ObjectSelectAll
    ),
    object_command!(
        "objects.focus_search",
        "keymap.objects.focus_search",
        &["secondary-f"],
        ObjectFocusSearch
    ),
    Command {
        id: "general.new_query",
        label_key: "keymap.general.new_query",
        group: CommandGroup::General,
        context: WORKSPACE_CONTEXT,
        defaults: &["ctrl-q"],
        // ⌘Q quits on macOS.
        mac_defaults: Some(&["cmd-shift-n"]),
        bind: bind!(NewQueryTab),
    },
    nav_command!(
        CommandGroup::General,
        PAGE_CONTEXT,
        "general.refresh_page",
        "keymap.general.refresh_page",
        &["f5"],
        RefreshPage
    ),
    nav_command!(
        CommandGroup::General,
        WORKSPACE_CONTEXT,
        "general.close_tab",
        "keymap.general.close_tab",
        &["secondary-w"],
        CloseActiveTab
    ),
    // The tab menu's 关闭其他选项卡 / 全部关闭, on the active tab.
    nav_command!(
        CommandGroup::General,
        WORKSPACE_CONTEXT,
        "general.close_other_tabs",
        "tab.close_other",
        &["secondary-alt-w"],
        CloseOtherTabs
    ),
    nav_command!(
        CommandGroup::General,
        WORKSPACE_CONTEXT,
        "general.close_all_tabs",
        "tab.close_all",
        &["secondary-shift-w"],
        CloseAllTabs
    ),
    nav_command!(
        CommandGroup::General,
        WORKSPACE_CONTEXT,
        "general.next_tab",
        "keymap.general.next_tab",
        &["ctrl-tab", "ctrl-pagedown"],
        NextTab
    ),
    nav_command!(
        CommandGroup::General,
        WORKSPACE_CONTEXT,
        "general.prev_tab",
        "keymap.general.prev_tab",
        &["ctrl-shift-tab", "ctrl-pageup"],
        PrevTab
    ),
    grid_command!(
        "grid.edit_value",
        "keymap.grid.edit_value",
        &["f2"],
        GridEditValue
    ),
    grid_command!("grid.refresh", "keymap.grid.refresh", &["f5"], GridRefresh),
    grid_command!(
        "grid.duplicate_row",
        "keymap.grid.duplicate_row",
        &["secondary-d"],
        GridDuplicateRow
    ),
    grid_command!(
        "grid.page_up",
        "keymap.grid.page_up",
        &["pageup"],
        GridPageUp
    ),
    grid_command!(
        "grid.page_down",
        "keymap.grid.page_down",
        &["pagedown"],
        GridPageDown
    ),
    grid_command!(
        "grid.select_page_up",
        "keymap.grid.select_page_up",
        &["shift-pageup"],
        GridSelectPageUp
    ),
    grid_command!(
        "grid.select_page_down",
        "keymap.grid.select_page_down",
        &["shift-pagedown"],
        GridSelectPageDown
    ),
    grid_command!(
        "grid.row_start",
        "keymap.grid.row_start",
        &["home"],
        GridRowStart
    ),
    grid_command!("grid.row_end", "keymap.grid.row_end", &["end"], GridRowEnd),
    grid_command!(
        "grid.first_cell",
        "keymap.grid.first_cell",
        &["secondary-home"],
        GridFirstCell
    ),
    grid_command!(
        "grid.last_cell",
        "keymap.grid.last_cell",
        &["secondary-end"],
        GridLastCell
    ),
    grid_command!(
        "grid.next_cell",
        "keymap.grid.next_cell",
        &["tab"],
        GridNextCell
    ),
    grid_command!(
        "grid.prev_cell",
        "keymap.grid.prev_cell",
        &["shift-tab"],
        GridPrevCell
    ),
    Command {
        id: "form.save",
        label_key: "keymap.form.save",
        group: CommandGroup::TableGrid,
        context: ROW_FORM_CONTEXT,
        defaults: &["secondary-s"],
        mac_defaults: None,
        bind: bind!(SaveRowForm),
    },
    editor_command!(
        "query.toggle_comment",
        "keymap.query.toggle_comment",
        &["secondary-k", "secondary-/"],
        ToggleComment
    ),
    editor_command!(
        "query.duplicate",
        "keymap.query.duplicate",
        &["secondary-d"],
        DuplicateLines
    ),
    editor_command!(
        "query.swap_line_up",
        "keymap.query.swap_line_up",
        &["secondary-t"],
        SwapLineUp
    ),
    editor_command!(
        "query.move_lines_up",
        "keymap.query.move_lines_up",
        &["alt-up"],
        MoveLinesUp
    ),
    editor_command!(
        "query.move_lines_down",
        "keymap.query.move_lines_down",
        &["alt-down"],
        MoveLinesDown
    ),
    Command {
        id: "query.run",
        label_key: "query.run",
        group: CommandGroup::QueryEditor,
        context: QUERY_EDITOR_CONTEXT,
        defaults: &["secondary-r"],
        mac_defaults: None,
        bind: bind!(RunQuery),
    },
    Command {
        id: "query.run_selected",
        label_key: "query.run_selected",
        group: CommandGroup::QueryEditor,
        context: QUERY_EDITOR_CONTEXT,
        defaults: &["secondary-shift-r", "secondary-enter"],
        mac_defaults: None,
        bind: bind!(RunSelectedQuery),
    },
    Command {
        id: "query.save",
        label_key: "keymap.query.save",
        group: CommandGroup::QueryEditor,
        context: QUERY_EDITOR_CONTEXT,
        defaults: &["secondary-s"],
        mac_defaults: None,
        bind: bind!(SaveQuery),
    },
    Command {
        id: "query.show_completions",
        label_key: "keymap.query.show_completions",
        group: CommandGroup::QueryEditor,
        context: QUERY_EDITOR_CONTEXT,
        defaults: &["ctrl-space"],
        mac_defaults: None,
        bind: bind!(ShowCompletions),
    },
    Command {
        id: "query.select_all_occurrences",
        label_key: "keymap.query.select_all_occurrences",
        group: CommandGroup::QueryEditor,
        context: QUERY_EDITOR_CONTEXT,
        defaults: &["secondary-shift-l"],
        mac_defaults: None,
        bind: bind!(SelectAllOccurrences),
    },
    Command {
        id: "designer.save",
        label_key: "keymap.designer.save",
        group: CommandGroup::Designer,
        context: DESIGNER_CONTEXT,
        defaults: &["secondary-s"],
        mac_defaults: None,
        bind: bind!(SaveDesign),
    },
    Command {
        id: "designer.select_all",
        label_key: "keymap.designer.select_all",
        group: CommandGroup::Designer,
        context: DESIGNER_CONTEXT,
        defaults: &["secondary-a"],
        mac_defaults: None,
        bind: bind!(DesignerSelectAll),
    },
    Command {
        id: "designer.copy",
        label_key: "keymap.designer.copy",
        group: CommandGroup::Designer,
        context: DESIGNER_CONTEXT,
        defaults: &["secondary-c"],
        mac_defaults: None,
        bind: bind!(DesignerCopy),
    },
    Command {
        id: "designer.paste",
        label_key: "keymap.designer.paste",
        group: CommandGroup::Designer,
        context: DESIGNER_CONTEXT,
        defaults: &["secondary-v"],
        mac_defaults: None,
        bind: bind!(DesignerPaste),
    },
    Command {
        id: "backups.rename",
        label_key: "keymap.backups.rename",
        group: CommandGroup::Lists,
        context: BACKUP_LIST_CONTEXT,
        defaults: &["f2"],
        mac_defaults: None,
        bind: bind!(RenameBackupFile),
    },
    Command {
        id: "backups.copy",
        label_key: "keymap.backups.copy",
        group: CommandGroup::Lists,
        context: BACKUP_LIST_CONTEXT,
        defaults: &["secondary-c"],
        mac_defaults: None,
        bind: bind!(CopyBackupFile),
    },
    Command {
        id: "backups.paste",
        label_key: "keymap.backups.paste",
        group: CommandGroup::Lists,
        context: BACKUP_LIST_CONTEXT,
        defaults: &["secondary-v"],
        mac_defaults: None,
        bind: bind!(PasteBackupFile),
    },
    Command {
        id: "backups.select_all",
        label_key: "keymap.backups.select_all",
        group: CommandGroup::Lists,
        context: BACKUP_LIST_CONTEXT,
        defaults: &["secondary-a"],
        mac_defaults: None,
        bind: bind!(SelectAllBackups),
    },
    Command {
        id: "backups.focus_search",
        label_key: "keymap.backups.focus_search",
        group: CommandGroup::Lists,
        context: BACKUP_LIST_CONTEXT,
        defaults: &["secondary-f"],
        mac_defaults: None,
        bind: bind!(BackupFocusSearch),
    },
    Command {
        id: "queries.rename",
        label_key: "keymap.queries.rename",
        group: CommandGroup::Lists,
        context: QUERY_LIST_CONTEXT,
        defaults: &["f2"],
        mac_defaults: None,
        bind: bind!(RenameQueryFile),
    },
    Command {
        id: "queries.copy",
        label_key: "keymap.queries.copy",
        group: CommandGroup::Lists,
        context: QUERY_LIST_CONTEXT,
        defaults: &["secondary-c"],
        mac_defaults: None,
        bind: bind!(CopyQueryFile),
    },
    Command {
        id: "queries.paste",
        label_key: "keymap.queries.paste",
        group: CommandGroup::Lists,
        context: QUERY_LIST_CONTEXT,
        defaults: &["secondary-v"],
        mac_defaults: None,
        bind: bind!(PasteQueryFile),
    },
    Command {
        id: "queries.select_all",
        label_key: "keymap.queries.select_all",
        group: CommandGroup::Lists,
        context: QUERY_LIST_CONTEXT,
        defaults: &["secondary-a"],
        mac_defaults: None,
        bind: bind!(SelectAllQueries),
    },
    Command {
        id: "queries.focus_search",
        label_key: "keymap.queries.focus_search",
        group: CommandGroup::Lists,
        context: QUERY_LIST_CONTEXT,
        defaults: &["secondary-f"],
        mac_defaults: None,
        bind: bind!(QueryFocusSearch),
    },
    Command {
        id: "users.focus_search",
        label_key: "keymap.users.focus_search",
        group: CommandGroup::Lists,
        context: USER_LIST_CONTEXT,
        defaults: &["secondary-f"],
        mac_defaults: None,
        bind: bind!(UserFocusSearch),
    },
    Command {
        id: "transfers.open",
        label_key: "keymap.transfers.open",
        group: CommandGroup::Lists,
        context: TRANSFER_LIST_CONTEXT,
        defaults: &["enter"],
        mac_defaults: None,
        bind: bind!(OpenTransferConfig),
    },
    Command {
        id: "transfers.delete",
        label_key: "keymap.transfers.delete",
        group: CommandGroup::Lists,
        context: TRANSFER_LIST_CONTEXT,
        defaults: &["delete"],
        mac_defaults: None,
        bind: bind!(DeleteTransferConfig),
    },
    Command {
        id: "transfers.select_all",
        label_key: "keymap.transfers.select_all",
        group: CommandGroup::Lists,
        context: TRANSFER_LIST_CONTEXT,
        defaults: &["secondary-a"],
        mac_defaults: None,
        bind: bind!(SelectAllTransfers),
    },
    Command {
        id: "status_sql.copy",
        label_key: "keymap.status_sql.copy",
        group: CommandGroup::Lists,
        context: GRID_STATUS_CONTEXT,
        defaults: &["secondary-c"],
        mac_defaults: None,
        bind: bind!(CopyStatusSql),
    },
    Command {
        id: "status_sql.select_all",
        label_key: "keymap.status_sql.select_all",
        group: CommandGroup::Lists,
        context: GRID_STATUS_CONTEXT,
        defaults: &["secondary-a"],
        mac_defaults: None,
        bind: bind!(SelectAllStatusSql),
    },
];

/// The command with `id`, if it exists.
pub(super) fn command(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// The keystrokes `command` is bound to: the user's override, or its built-in ones.
pub(super) fn keys_for(
    command: &Command,
    overrides: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    match overrides.get(command.id) {
        Some(keys) => keys.clone(),
        None => command
            .default_keys()
            .iter()
            .map(|key| key.to_string())
            .collect(),
    }
}

/// Store `keys` as `command`'s binding in `overrides`: equal to the built-in keys means "no
/// override", so a reset command stops being listed in `settings.json`.
pub(super) fn set_keys(
    command: &Command,
    keys: Vec<String>,
    overrides: &mut BTreeMap<String, Vec<String>>,
) {
    let defaults: Vec<String> = command
        .default_keys()
        .iter()
        .map(|key| key.to_string())
        .collect();
    if keys == defaults {
        overrides.remove(command.id);
    } else {
        overrides.insert(command.id.to_string(), keys);
    }
}

/// A keystroke in a canonical form for comparison (`secondary-r` and `ctrl-r` are the same key on
/// Windows), or `None` when it does not parse.
fn canonical(keystroke: &str, os: OperatingSystem) -> Option<String> {
    Keystroke::parse_with_os(keystroke, os).ok().map(|key| {
        let modifiers = key.modifiers;
        let mut parts = Vec::new();
        if modifiers.control {
            parts.push("ctrl");
        }
        if modifiers.alt {
            parts.push("alt");
        }
        if modifiers.shift {
            parts.push("shift");
        }
        if modifiers.platform {
            parts.push("cmd");
        }
        if modifiers.function {
            parts.push("fn");
        }
        parts.push(&key.key);
        parts.join("-")
    })
}

/// The other commands that `command` shares a keystroke with in the same key context, given
/// `overrides`. Only one of them would fire, so the 快捷键 page flags them.
pub(super) fn conflicts(
    command: &Command,
    overrides: &BTreeMap<String, Vec<String>>,
    os: OperatingSystem,
) -> Vec<&'static Command> {
    let mine: Vec<String> = keys_for(command, overrides)
        .iter()
        .filter_map(|key| canonical(key, os))
        .collect();
    if mine.is_empty() {
        return Vec::new();
    }
    COMMANDS
        .iter()
        .filter(|other| other.id != command.id && other.context == command.context)
        .filter(|other| {
            keys_for(other, overrides)
                .iter()
                .filter_map(|key| canonical(key, os))
                .any(|key| mine.contains(&key))
        })
        .collect()
}

/// The gpui key context a command binds in: none for workspace-wide commands, so they fire
/// whatever has focus.
fn binding_context(context: &'static str) -> Option<&'static str> {
    (context != WORKSPACE_CONTEXT).then_some(context)
}

/// The keystrokes [`bind_keymap`] registered, each with its key context, so the next call can
/// shadow them.
pub(super) type BoundKeys = Vec<(String, &'static str)>;

/// Bind every command to its keystrokes given `overrides`, first shadowing `previous` (what the
/// last call bound) with `NoAction`. A keystroke that does not parse is skipped rather than
/// panicking. Returns what is now bound.
pub(super) fn bind_keymap(
    cx: &mut App,
    overrides: &BTreeMap<String, Vec<String>>,
    previous: &[(String, &'static str)],
) -> BoundKeys {
    let os = cx.platform_info().os;
    let mut bindings = Vec::new();
    for (keystroke, context) in previous {
        if let Some(binding) = load_binding(keystroke, NoAction {}, binding_context(context), cx) {
            bindings.push(binding);
        }
    }
    let mut bound = Vec::new();
    for command in COMMANDS {
        for keystroke in keys_for(command, overrides) {
            if Keystroke::parse_with_os(&keystroke, os).is_err() {
                continue;
            }
            if let Some(binding) = (command.bind)(&keystroke, binding_context(command.context), cx)
            {
                bindings.push(binding);
                bound.push((keystroke, command.context));
            }
        }
    }
    cx.bind_keys(bindings);
    bound
}

#[cfg(test)]
mod tests {
    use super::*;

    const OS: OperatingSystem = OperatingSystem::Windows;

    fn keys(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn command_ids_are_unique_and_defaults_parse() {
        for (index, command) in COMMANDS.iter().enumerate() {
            assert!(
                COMMANDS[index + 1..]
                    .iter()
                    .all(|other| other.id != command.id),
                "duplicate command id {}",
                command.id
            );
            for key in command
                .defaults
                .iter()
                .chain(command.mac_defaults.unwrap_or(&[]))
            {
                assert!(
                    Keystroke::parse_with_os(key, OS).is_ok(),
                    "{}: bad default {key}",
                    command.id
                );
            }
        }
    }

    #[test]
    fn built_in_keystrokes_do_not_conflict() {
        let none = BTreeMap::new();
        for command in COMMANDS {
            let clashes: Vec<&str> = conflicts(command, &none, OS).iter().map(|c| c.id).collect();
            assert!(
                clashes.is_empty(),
                "{} clashes with {clashes:?}",
                command.id
            );
        }
    }

    #[test]
    fn an_override_replaces_the_defaults_and_resetting_removes_it() {
        let run = command("query.run").expect("query.run");
        let mut overrides = BTreeMap::new();
        set_keys(run, keys(&["f9"]), &mut overrides);
        assert_eq!(keys_for(run, &overrides), keys(&["f9"]));

        set_keys(run, keys(&["secondary-r"]), &mut overrides);
        assert!(overrides.is_empty());
        assert_eq!(keys_for(run, &overrides), keys(&["secondary-r"]));
    }

    #[test]
    fn an_empty_override_unbinds_the_command() {
        let save = command("query.save").expect("query.save");
        let mut overrides = BTreeMap::new();
        set_keys(save, Vec::new(), &mut overrides);
        assert!(keys_for(save, &overrides).is_empty());
        assert!(conflicts(save, &overrides, OS).is_empty());
    }

    #[test]
    fn the_same_key_in_one_context_is_a_conflict_but_not_across_contexts() {
        let run = command("query.run").expect("query.run");
        let save = command("query.save").expect("query.save");
        let designer_save = command("designer.save").expect("designer.save");
        let mut overrides = BTreeMap::new();
        set_keys(run, keys(&["secondary-s"]), &mut overrides);

        let clashes: Vec<&str> = conflicts(run, &overrides, OS)
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(clashes, vec!["query.save"]);
        assert!(
            conflicts(save, &overrides, OS)
                .iter()
                .any(|c| c.id == "query.run")
        );
        // designer.save also uses Ctrl/⌘+S, but in the designer's context.
        assert!(conflicts(designer_save, &overrides, OS).is_empty());
    }

    /// gpui-kit 0.8 parses a runtime keystroke against the host OS, so a rebindable
    /// `secondary-…` command must resolve to Ctrl (⌘ on macOS) when it is bound.
    #[gpui::test]
    fn a_secondary_binding_resolves_to_the_platform_modifier(cx: &mut gpui::TestAppContext) {
        use gpui::AsKeystroke as _;

        cx.update(|cx| {
            let binding = load_binding("secondary-r", TreeUp, Some(TREE_CONTEXT), cx)
                .expect("secondary-r parses");
            let key = binding
                .keystrokes()
                .first()
                .expect("one keystroke")
                .as_keystroke();
            assert_eq!(key.key, "r");
            match cx.platform_info().os {
                OperatingSystem::Mac | OperatingSystem::Ios => assert!(key.modifiers.platform),
                _ => assert!(key.modifiers.control),
            }
        });
    }

    /// A keystroke that does not parse is skipped, not bound, and never panics.
    #[gpui::test]
    fn an_unparseable_binding_is_skipped(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            assert!(load_binding("ctrl-a-b", TreeUp, Some(TREE_CONTEXT), cx).is_none());
        });
    }
}
