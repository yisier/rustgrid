use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, ClipboardItem, Context, FontWeight, SharedString, div, px, rgb, svg};
use gpui_kit::component::Sizable;
use gpui_kit::component::switch::Switch;
use rust_i18n::t;
use rustgrid_config::{
    ConfigStore, DEFAULT_MCP_MAX_ROWS, DEFAULT_MCP_TIMEOUT_SECONDS, MCP_CONFIG_VERSION, McpConfig,
    McpConnectionPermission,
};

use super::AppView;
use super::options::*;
use crate::app::ui::{TextArea, TextAreaOptions, TextInput, TextInputOptions};
use crate::session::Loadable;

/// Staged MCP settings for the Options window. Nothing here reaches `mcp.json` until 确定 (or the
/// per-connection editor's 保存 stages it into [`permissions`](Self::permissions)).
#[derive(Debug, Clone)]
pub(super) struct McpOptionsState {
    pub enabled: bool,
    pub max_rows: String,
    pub timeout_seconds: String,
    pub permissions: BTreeMap<String, McpConnectionPermission>,
    pub editing: Option<McpConnectionEditState>,
    pub copied: Option<String>,
    pub error: Option<String>,
}

impl Default for McpOptionsState {
    fn default() -> Self {
        Self {
            enabled: false,
            max_rows: DEFAULT_MCP_MAX_ROWS.to_string(),
            timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS.to_string(),
            permissions: BTreeMap::new(),
            editing: None,
            copied: None,
            error: None,
        }
    }
}

impl McpOptionsState {
    pub(super) fn load(config: &ConfigStore) -> Self {
        match config.load_mcp() {
            Ok(mcp) => Self {
                enabled: mcp.enabled,
                max_rows: mcp.max_rows.to_string(),
                timeout_seconds: mcp.timeout_seconds.to_string(),
                permissions: mcp.connections,
                editing: None,
                copied: None,
                error: None,
            },
            Err(error) => Self {
                error: Some(format!("Could not read mcp.json: {error}")),
                ..Self::default()
            },
        }
    }

    /// The staged permission for a profile, or the safe default (disabled, read-only).
    fn permission(&self, profile_id: &str) -> McpConnectionPermission {
        self.permissions
            .get(profile_id)
            .cloned()
            .unwrap_or_default()
    }

    pub(super) fn save(&self, store: &ConfigStore) -> Result<(), String> {
        let max_rows = self
            .max_rows
            .trim()
            .parse::<u32>()
            .unwrap_or(DEFAULT_MCP_MAX_ROWS);
        let timeout_seconds = self
            .timeout_seconds
            .trim()
            .parse::<u32>()
            .unwrap_or(DEFAULT_MCP_TIMEOUT_SECONDS)
            .clamp(1, 600);
        let config = McpConfig {
            version: MCP_CONFIG_VERSION,
            enabled: self.enabled,
            max_rows,
            timeout_seconds,
            connections: self.permissions.clone(),
        };
        store.save_mcp(&config).map_err(|error| error.to_string())
    }
}

/// The staged editor for one connection's rules.
#[derive(Debug, Clone)]
pub(super) struct McpConnectionEditState {
    pub profile_id: String,
    pub name: String,
    pub driver: String,
    pub enabled: bool,
    pub read_only: bool,
    pub all_databases: bool,
    pub selected_databases: BTreeSet<String>,
    pub available_databases: Vec<String>,
    pub blacklisted_tables: String,
    pub context_prompt: String,
    pub confirm_read_write: bool,
    pub error: Option<String>,
}

impl AppView {
    fn ensure_mcp_max_rows_input(&mut self, value: String, cx: &mut Context<'_, Self>) {
        if self.options_mcp_max_rows_input.is_some() {
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                    let _ = weak.update(cx, |app, cx| {
                        app.mcp_options.max_rows = digits;
                        cx.notify();
                    });
                },
            ))
        });
        self.options_mcp_max_rows_input = Some(input);
        cx.notify();
    }

    fn ensure_mcp_timeout_input(&mut self, value: String, cx: &mut Context<'_, Self>) {
        if self.options_mcp_timeout_input.is_some() {
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                    let _ = weak.update(cx, |app, cx| {
                        app.mcp_options.timeout_seconds = digits;
                        cx.notify();
                    });
                },
            ))
        });
        self.options_mcp_timeout_input = Some(input);
        cx.notify();
    }

    /// The AI / MCP page: the global switch, the stdio client configuration, and the per-connection
    /// permission matrix (or the editor for one connection).
    pub(super) fn options_mcp_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        if let Some(editing) = self.mcp_options.editing.clone() {
            return self.render_mcp_editor(&editing, cx);
        }
        self.render_mcp_page(cx)
    }

    fn render_mcp_page(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        if self.options_mcp_max_rows_input.is_none() {
            let weak = cx.weak_entity();
            let value = self.mcp_options.max_rows.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |app, cx| app.ensure_mcp_max_rows_input(value, cx));
            });
        }
        if self.options_mcp_timeout_input.is_none() {
            let weak = cx.weak_entity();
            let value = self.mcp_options.timeout_seconds.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |app, cx| app.ensure_mcp_timeout_input(value, cx));
            });
        }

        let global_card = settings_card(theme)
            .child(
                toggle_row(
                    theme,
                    "options-mcp-global-enabled",
                    t!("options.mcp.global_enabled").to_string(),
                    Some(t!("options.mcp.global_enabled_hint").to_string()),
                    self.mcp_options.enabled,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.mcp_options.enabled = !this.mcp_options.enabled;
                    cx.notify();
                })),
            )
            .child(setting_row(
                "options-mcp-max-rows",
                theme,
                t!("options.mcp.default_max_rows").to_string(),
                Some(t!("options.mcp.default_max_rows_hint").to_string()),
                div()
                    .w(px(90.0))
                    .flex_none()
                    .children(self.options_mcp_max_rows_input.clone())
                    .into_any_element(),
                true,
            ))
            .child(setting_row(
                "options-mcp-timeout",
                theme,
                t!("options.mcp.timeout").to_string(),
                Some(t!("options.mcp.timeout_hint").to_string()),
                div()
                    .w(px(90.0))
                    .flex_none()
                    .children(self.options_mcp_timeout_input.clone())
                    .into_any_element(),
                true,
            ))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("options.mcp.security_warning").to_string()),
            );

        let exe = std::env::current_exe()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| "RustGrid".to_string());
        let mcp_servers = serde_json::json!({
            "mcpServers": {
                "rustgrid": {
                    "command": exe,
                    "args": ["--mcp-stdio"]
                }
            }
        });
        let mcp_servers_text =
            serde_json::to_string_pretty(&mcp_servers).unwrap_or_else(|_| "{}".to_string());
        let generic = serde_json::json!({
            "command": std::env::current_exe()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| "RustGrid".to_string()),
            "args": ["--mcp-stdio"]
        });
        let generic_text =
            serde_json::to_string_pretty(&generic).unwrap_or_else(|_| "{}".to_string());

        let copy_button = |id: &'static str, label: String, text: String| {
            self.dialog_button(
                id,
                label,
                false,
                cx.listener(move |this, _event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    this.mcp_options.copied = Some(t!("options.mcp.copied").to_string());
                    cx.notify();
                }),
            )
        };
        let client_row = |name: String, id: &'static str, text: String| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_2()
                .border_b_1()
                .border_color(rgb(theme.border))
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(rgb(theme.text))
                        .child(name),
                )
                .child(copy_button(
                    id,
                    t!("options.mcp.copy_config").to_string(),
                    text,
                ))
        };

        let clients_card = settings_card(theme)
            .child(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(11.5))
                    .line_height(px(16.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("options.mcp.stdio_hint").to_string()),
            )
            .child(client_row(
                t!("options.mcp.claude").to_string(),
                "mcp-copy-claude",
                mcp_servers_text.clone(),
            ))
            .child(client_row(
                t!("options.mcp.cursor").to_string(),
                "mcp-copy-cursor",
                mcp_servers_text.clone(),
            ))
            .child(client_row(
                t!("options.mcp.custom").to_string(),
                "mcp-copy-generic",
                generic_text,
            ))
            .child(
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(11.0))
                    .text_color(rgb(theme.primary))
                    .children(self.mcp_options.copied.clone()),
            );

        let mut matrix_rows = Vec::new();
        if self.connections.is_empty() {
            matrix_rows.push(
                div()
                    .p_3()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("options.mcp.matrix_empty").to_string())
                    .into_any_element(),
            );
        } else {
            matrix_rows.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .bg(rgb(theme.header_bg))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(theme.text_muted))
                    .child(
                        div()
                            .w(px(160.0))
                            .truncate()
                            .child(t!("options.mcp.matrix_name").to_string()),
                    )
                    .child(
                        div()
                            .w(px(70.0))
                            .truncate()
                            .child(t!("options.mcp.matrix_status").to_string()),
                    )
                    .child(
                        div()
                            .w(px(80.0))
                            .truncate()
                            .child(t!("options.mcp.matrix_mode").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .truncate()
                            .child(t!("options.mcp.matrix_scope").to_string()),
                    )
                    .child(
                        div()
                            .w(px(60.0))
                            .flex()
                            .justify_end()
                            .child(t!("options.mcp.matrix_action").to_string()),
                    )
                    .into_any_element(),
            );

            for (index, node) in self.connections.iter().enumerate() {
                let permission = self.mcp_options.permission(&node.profile.id);
                let profile_id = node.profile.id.clone();
                let profile_id_toggle = profile_id.clone();
                let profile_id_edit = profile_id.clone();
                let profile_id_scope = profile_id.clone();
                let switch_id = SharedString::from(format!("mcp-switch-{profile_id_toggle}"));
                let last = index + 1 == self.connections.len();

                let mode_element = if permission.enabled {
                    div()
                        .id(SharedString::from(format!("mcp-mode-{profile_id}")))
                        .cursor_pointer()
                        .text_size(px(11.5))
                        .text_color(rgb(if permission.read_only {
                            theme.text
                        } else {
                            theme.danger
                        }))
                        .hover(|el| el.underline())
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.open_mcp_editor(&profile_id_edit, cx);
                        }))
                        .child(if permission.read_only {
                            t!("options.mcp.matrix_readonly").to_string()
                        } else {
                            t!("options.mcp.matrix_readwrite").to_string()
                        })
                        .into_any_element()
                } else {
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child("-")
                        .into_any_element()
                };

                let scope_element = if permission.enabled {
                    let text = if permission.allowed_databases.is_empty() {
                        t!("options.mcp.matrix_all_db").to_string()
                    } else if permission.allowed_databases.len() == 1 {
                        permission.allowed_databases[0].clone()
                    } else {
                        t!(
                            "options.mcp.matrix_count_db",
                            count = permission.allowed_databases.len()
                        )
                        .to_string()
                    };
                    div()
                        .id(SharedString::from(format!("mcp-scope-{profile_id_scope}")))
                        .cursor_pointer()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(rgb(if permission.allowed_databases.is_empty() {
                            theme.text_muted
                        } else {
                            theme.primary
                        }))
                        .hover(|el| el.underline())
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.open_mcp_editor(&profile_id_scope, cx);
                        }))
                        .child(text)
                        .into_any_element()
                } else {
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child("-")
                        .into_any_element()
                };

                matrix_rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py_2()
                        .when(!last, |row| {
                            row.border_b_1().border_color(rgb(theme.border))
                        })
                        .child(
                            div()
                                .w(px(160.0))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .text_size(px(12.5))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(rgb(theme.text))
                                        .truncate()
                                        .child(node.profile.name.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.0))
                                        .text_color(rgb(theme.text_muted))
                                        .truncate()
                                        .child(node.profile.driver.as_str().to_string()),
                                ),
                        )
                        .child(
                            div().w(px(70.0)).flex().items_center().child(
                                div()
                                    .id(SharedString::from(format!(
                                        "mcp-toggle-{profile_id_toggle}"
                                    )))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        let permission = this
                                            .mcp_options
                                            .permissions
                                            .entry(profile_id_toggle.clone())
                                            .or_default();
                                        permission.enabled = !permission.enabled;
                                        cx.notify();
                                    }))
                                    .child(
                                        Switch::new(switch_id)
                                            .checked(permission.enabled)
                                            .with_size(gpui_kit::component::Size::Small)
                                            .tab_stop(false),
                                    ),
                            ),
                        )
                        .child(div().w(px(80.0)).flex().items_center().child(mode_element))
                        .child(div().flex_1().min_w(px(0.0)).child(scope_element))
                        .child(
                            div().w(px(60.0)).flex().justify_end().child(
                                div()
                                    .id(SharedString::from(format!("mcp-edit-{profile_id}")))
                                    .cursor_pointer()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.0))
                                    .text_size(px(11.5))
                                    .text_color(rgb(theme.primary))
                                    .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                                    .on_click(cx.listener(move |this, _event, _window, cx| {
                                        this.open_mcp_editor(&profile_id, cx);
                                    }))
                                    .child(t!("options.mcp.matrix_edit").to_string()),
                            ),
                        )
                        .into_any_element(),
                );
            }
        }
        let matrix_card = settings_card(theme).children(matrix_rows);

        div()
            .relative()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .when_some(self.mcp_options.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.danger))
                        .child(error),
                )
            })
            .child(section_label(
                theme,
                t!("options.mcp.global_section").to_string(),
            ))
            .child(global_card)
            .child(section_label(
                theme,
                t!("options.mcp.clients_section").to_string(),
            ))
            .child(clients_card)
            .child(section_label(
                theme,
                t!("options.mcp.matrix_section").to_string(),
            ))
            .child(
                div()
                    .px(px(2.0))
                    .text_size(px(11.0))
                    .text_color(rgb(theme.text_muted))
                    .child(t!("options.mcp.matrix_hint").to_string()),
            )
            .child(matrix_card)
            .into_any_element()
    }

    fn open_mcp_editor(&mut self, profile_id: &str, cx: &mut Context<'_, Self>) {
        let Some(node) = self
            .connections
            .iter()
            .find(|node| node.profile.id == profile_id)
        else {
            return;
        };
        let permission = self.mcp_options.permission(profile_id);
        let mut available = BTreeSet::new();
        if let Loadable::Loaded(ref databases) = node.databases {
            for database in databases {
                if !database.name.trim().is_empty() {
                    available.insert(database.name.clone());
                }
            }
        }
        for database in &permission.allowed_databases {
            if !database.trim().is_empty() {
                available.insert(database.clone());
            }
        }
        let dialect = self
            .registry
            .get(&node.profile.driver)
            .map(|driver| driver.dialect_for(&node.profile))
            .unwrap_or_default();
        match dialect {
            rustgrid_core::DriverDialect::Oracle => {}
            rustgrid_core::DriverDialect::Sqlite => {
                available.insert("main".to_string());
            }
            _ => {
                if let Some(database) = &node.profile.database
                    && !database.trim().is_empty()
                {
                    available.insert(database.clone());
                }
            }
        }

        let blacklist = permission.blacklisted_tables.join(", ");
        let prompt = permission.context_prompt.clone();
        let editing = McpConnectionEditState {
            profile_id: node.profile.id.clone(),
            name: node.profile.name.clone(),
            driver: node.profile.driver.as_str().to_string(),
            enabled: permission.enabled,
            read_only: permission.read_only,
            all_databases: permission.allowed_databases.is_empty(),
            selected_databases: permission.allowed_databases.iter().cloned().collect(),
            available_databases: available.into_iter().collect(),
            blacklisted_tables: blacklist.clone(),
            context_prompt: prompt.clone(),
            confirm_read_write: !permission.read_only,
            error: None,
        };
        self.mcp_options.editing = Some(editing);
        self.create_mcp_editor_inputs(blacklist, prompt, cx);
        cx.notify();
    }

    fn create_mcp_editor_inputs(
        &mut self,
        blacklist: String,
        prompt: String,
        cx: &mut Context<'_, Self>,
    ) {
        let theme = self.theme;
        let scope = cx.new(move |cx| {
            TextInput::new(
                theme,
                String::new(),
                TextInputOptions {
                    placeholder: t!("options.mcp.scope_manual_placeholder")
                        .to_string()
                        .into(),
                    ..Default::default()
                },
                cx,
            )
        });
        self.options_mcp_scope_input = Some(scope);

        let weak = cx.weak_entity();
        let blacklist_input = cx.new(move |cx| {
            TextInput::new(
                theme,
                blacklist,
                TextInputOptions {
                    placeholder: t!("options.mcp.blacklist_placeholder").to_string().into(),
                    ..Default::default()
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let text = text.to_string();
                let _ = weak.update(cx, |app, cx| {
                    if let Some(editing) = app.mcp_options.editing.as_mut() {
                        editing.blacklisted_tables = text;
                    }
                    cx.notify();
                });
            }))
        });
        self.options_mcp_blacklist_input = Some(blacklist_input);

        let weak = cx.weak_entity();
        let prompt_input = cx.new(move |cx| {
            TextArea::new(
                theme,
                prompt,
                TextAreaOptions {
                    json: false,
                    line_numbers: false,
                    placeholder: t!("options.mcp.context_prompt_placeholder")
                        .to_string()
                        .into(),
                    text_size: 12.0,
                },
                cx,
            )
            .on_change(Rc::new(move |text, _window, cx| {
                let text = text.to_string();
                let _ = weak.update(cx, |app, cx| {
                    if let Some(editing) = app.mcp_options.editing.as_mut() {
                        editing.context_prompt = text;
                    }
                    cx.notify();
                });
            }))
        });
        self.options_mcp_prompt_input = Some(prompt_input);
    }

    fn close_mcp_editor(&mut self, cx: &mut Context<'_, Self>) {
        self.mcp_options.editing = None;
        self.options_mcp_scope_input = None;
        self.options_mcp_blacklist_input = None;
        self.options_mcp_prompt_input = None;
        cx.notify();
    }

    /// Read the editor's live fields, validate them, and stage the permission. Used by the editor's
    /// 保存 and by the Options window's 确定/应用, so typing and immediately confirming never loses
    /// the last keystroke.
    pub(super) fn commit_mcp_editor(&mut self, cx: &mut Context<'_, Self>) -> Result<(), String> {
        let Some(editing) = self.mcp_options.editing.clone() else {
            return Ok(());
        };
        let manual = self
            .options_mcp_scope_input
            .as_ref()
            .and_then(|input| input.read(cx).current_text(cx))
            .unwrap_or_default();
        let blacklist = self
            .options_mcp_blacklist_input
            .as_ref()
            .and_then(|input| input.read(cx).current_text(cx))
            .unwrap_or_else(|| editing.blacklisted_tables.clone());
        let prompt = self
            .options_mcp_prompt_input
            .as_ref()
            .and_then(|input| input.read(cx).current_text(cx))
            .unwrap_or_else(|| editing.context_prompt.clone());

        if !editing.read_only && !editing.confirm_read_write {
            return Err(t!("options.mcp.confirm_read_write_required").to_string());
        }

        let allowed_databases = if editing.all_databases {
            Vec::new()
        } else {
            let mut databases: Vec<String> = Vec::new();
            let manual = manual.split(',').map(str::trim).filter(|db| !db.is_empty());
            for database in editing
                .selected_databases
                .iter()
                .map(String::as_str)
                .chain(manual)
            {
                let database = database
                    .trim()
                    .trim_matches(|c| c == '`' || c == '"' || c == '[' || c == ']');
                if database.is_empty()
                    || databases
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(database))
                {
                    continue;
                }
                databases.push(database.to_string());
            }
            if databases.is_empty() {
                return Err(t!("options.mcp.scope_empty_error").to_string());
            }
            databases.sort();
            databases
        };

        let mut blacklisted_tables: Vec<String> = Vec::new();
        for entry in blacklist
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
        {
            if blacklisted_tables
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(entry))
            {
                continue;
            }
            blacklisted_tables.push(entry.to_string());
        }

        self.mcp_options.permissions.insert(
            editing.profile_id,
            McpConnectionPermission {
                enabled: editing.enabled,
                read_only: editing.read_only,
                allowed_databases,
                blacklisted_tables,
                context_prompt: prompt.trim().to_string(),
            },
        );
        self.mcp_options.editing = None;
        self.options_mcp_scope_input = None;
        self.options_mcp_blacklist_input = None;
        self.options_mcp_prompt_input = None;
        Ok(())
    }

    fn render_mcp_editor(
        &self,
        editing: &McpConnectionEditState,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let read_only_hint = t!("options.mcp.read_only_hint").to_string();
        let read_write_warning = t!("options.mcp.read_write_warning").to_string();

        let mode_buttons = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(self.dialog_button(
                "mcp-mode-readonly",
                t!("options.mcp.matrix_readonly").to_string(),
                editing.read_only,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(editing) = this.mcp_options.editing.as_mut() {
                        editing.read_only = true;
                        editing.confirm_read_write = false;
                        editing.error = None;
                    }
                    cx.notify();
                }),
            ))
            .child(self.dialog_button(
                "mcp-mode-readwrite",
                t!("options.mcp.matrix_readwrite").to_string(),
                !editing.read_only,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(editing) = this.mcp_options.editing.as_mut() {
                        if editing.read_only {
                            editing.confirm_read_write = false;
                        }
                        editing.read_only = false;
                        editing.error = None;
                    }
                    cx.notify();
                }),
            ));

        let scope_mode = div()
            .flex()
            .flex_row()
            .gap_2()
            .child(self.dialog_button(
                "mcp-scope-all",
                t!("options.mcp.scope_mode_all").to_string(),
                editing.all_databases,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(editing) = this.mcp_options.editing.as_mut() {
                        editing.all_databases = true;
                        editing.error = None;
                    }
                    cx.notify();
                }),
            ))
            .child(self.dialog_button(
                "mcp-scope-specific",
                t!("options.mcp.scope_mode_custom").to_string(),
                !editing.all_databases,
                cx.listener(|this, _event, _window, cx| {
                    if let Some(editing) = this.mcp_options.editing.as_mut() {
                        editing.all_databases = false;
                        editing.error = None;
                    }
                    cx.notify();
                }),
            ));

        let database_rows = editing
            .available_databases
            .iter()
            .map(|database| {
                let checked = editing.selected_databases.contains(database);
                let database_name = database.clone();
                div()
                    .id(SharedString::from(format!("mcp-db-{database}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        if let Some(editing) = this.mcp_options.editing.as_mut() {
                            if editing.selected_databases.contains(&database_name) {
                                editing.selected_databases.remove(&database_name);
                            } else {
                                editing.selected_databases.insert(database_name.clone());
                            }
                            editing.error = None;
                        }
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(14.0))
                            .h(px(14.0))
                            .rounded(px(3.0))
                            .border_1()
                            .border_color(rgb(if checked { theme.primary } else { theme.border }))
                            .bg(rgb(if checked {
                                theme.primary
                            } else {
                                theme.dialog_bg
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(checked, |el| {
                                el.child(
                                    svg()
                                        .path("icons/check.svg")
                                        .w(px(10.0))
                                        .h(px(10.0))
                                        .text_color(rgb(0xffffff)),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(rgb(theme.text))
                            .child(database.clone()),
                    )
            })
            .collect::<Vec<_>>();

        let scope_detail = if editing.all_databases {
            div()
                .px_4()
                .pb_3()
                .text_size(px(11.0))
                .text_color(rgb(theme.text_muted))
                .child(t!("options.mcp.scope_mode_all_hint").to_string())
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .pb_3()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(11.5))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(theme.text))
                                .child(t!("options.mcp.scope_available_list").to_string()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_3()
                                .child(
                                    div()
                                        .id("mcp-scope-select-all")
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(theme.primary))
                                        .hover(|el| el.underline())
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            if let Some(editing) = this.mcp_options.editing.as_mut()
                                            {
                                                for database in &editing.available_databases {
                                                    editing
                                                        .selected_databases
                                                        .insert(database.clone());
                                                }
                                                editing.error = None;
                                            }
                                            cx.notify();
                                        }))
                                        .child(t!("options.mcp.scope_select_all").to_string()),
                                )
                                .child(
                                    div()
                                        .id("mcp-scope-clear")
                                        .cursor_pointer()
                                        .text_size(px(11.0))
                                        .text_color(rgb(theme.text_muted))
                                        .hover(|el| el.underline())
                                        .on_click(cx.listener(|this, _event, _window, cx| {
                                            if let Some(editing) = this.mcp_options.editing.as_mut()
                                            {
                                                editing.selected_databases.clear();
                                                editing.error = None;
                                            }
                                            cx.notify();
                                        }))
                                        .child(t!("options.mcp.scope_clear").to_string()),
                                ),
                        ),
                )
                .when(!database_rows.is_empty(), |el| {
                    el.child(
                        div()
                            .id("mcp-scope-database-list")
                            .max_h(px(160.0))
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .p_2()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.dialog_face))
                            .children(database_rows),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(rgb(theme.text_muted))
                                .child(t!("options.mcp.scope_manual_label").to_string()),
                        )
                        .child(
                            div().w_full().child(
                                self.options_mcp_scope_input
                                    .clone()
                                    .map(|input| input.into_any_element())
                                    .unwrap_or_else(|| div().into_any_element()),
                            ),
                        ),
                )
                .into_any_element()
        };

        let read_write_block = if editing.read_only {
            div().into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_4()
                .pb_3()
                .child(
                    div()
                        .text_size(px(11.5))
                        .line_height(px(16.0))
                        .text_color(rgb(theme.danger))
                        .child(read_write_warning),
                )
                .child(
                    toggle_row(
                        theme,
                        "mcp-confirm-readwrite",
                        t!("options.mcp.confirm_read_write").to_string(),
                        None,
                        editing.confirm_read_write,
                        false,
                    )
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        if let Some(editing) = this.mcp_options.editing.as_mut() {
                            editing.confirm_read_write = !editing.confirm_read_write;
                            editing.error = None;
                        }
                        cx.notify();
                    })),
                )
                .into_any_element()
        };

        let error_block = editing
            .error
            .clone()
            .map(|error| {
                div()
                    .px_4()
                    .py_2()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.danger))
                    .child(error)
                    .into_any_element()
            })
            .unwrap_or_else(|| div().into_any_element());

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(self.dialog_button(
                        "mcp-editor-back",
                        t!("options.mcp.edit_back").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| {
                            this.close_mcp_editor(cx);
                        }),
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(theme.text))
                                    .child(
                                        t!("options.mcp.edit_title", name = editing.name.clone())
                                            .to_string(),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(editing.driver.clone()),
                            ),
                    ),
            )
            .child(
                settings_card(theme)
                    .child(
                        toggle_row(
                            theme,
                            "mcp-connection-enabled",
                            t!("options.mcp.enable_connection").to_string(),
                            None,
                            editing.enabled,
                            true,
                        )
                        .on_click(cx.listener(
                            |this, _event, _window, cx| {
                                if let Some(editing) = this.mcp_options.editing.as_mut() {
                                    editing.enabled = !editing.enabled;
                                }
                                cx.notify();
                            },
                        )),
                    )
                    .child(setting_row(
                        "mcp-connection-mode",
                        theme,
                        t!("options.mcp.mode").to_string(),
                        Some(read_only_hint),
                        mode_buttons.into_any_element(),
                        true,
                    ))
                    .child(read_write_block)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(rgb(theme.text))
                                            .child(t!("options.mcp.database_scope").to_string()),
                                    )
                                    .child(scope_mode),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("options.mcp.database_scope_hint").to_string()),
                            ),
                    )
                    .child(scope_detail)
                    .child(setting_row(
                        "mcp-connection-blacklist",
                        theme,
                        t!("options.mcp.blacklist").to_string(),
                        Some(t!("options.mcp.blacklist_hint").to_string()),
                        div()
                            .w(px(260.0))
                            .flex_none()
                            .child(
                                self.options_mcp_blacklist_input
                                    .clone()
                                    .map(|input| input.into_any_element())
                                    .unwrap_or_else(|| div().into_any_element()),
                            )
                            .into_any_element(),
                        true,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(theme.text))
                                    .child(t!("options.mcp.context_prompt").to_string()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .line_height(px(16.0))
                                    .text_color(rgb(theme.text_muted))
                                    .child(t!("options.mcp.context_prompt_hint").to_string()),
                            )
                            .child(
                                div().h(px(92.0)).w_full().child(
                                    self.options_mcp_prompt_input
                                        .clone()
                                        .map(|input| input.into_any_element())
                                        .unwrap_or_else(|| div().into_any_element()),
                                ),
                            ),
                    )
                    .child(error_block),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    .child(self.dialog_button(
                        "mcp-editor-cancel",
                        t!("options.mcp.scope_cancel").to_string(),
                        false,
                        cx.listener(|this, _event, _window, cx| {
                            this.close_mcp_editor(cx);
                        }),
                    ))
                    .child(self.dialog_button(
                        "mcp-editor-save",
                        t!("options.mcp.scope_save").to_string(),
                        true,
                        cx.listener(
                            |this, _event, _window, cx| match this.commit_mcp_editor(cx) {
                                Ok(()) => cx.notify(),
                                Err(error) => {
                                    if let Some(editing) = this.mcp_options.editing.as_mut() {
                                        editing.error = Some(error);
                                    }
                                    cx.notify();
                                }
                            },
                        ),
                    )),
            )
            .into_any_element()
    }
}
