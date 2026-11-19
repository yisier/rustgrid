use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::prelude::*;
use gpui::{AnyElement, ClipboardItem, Context, FontWeight, SharedString, div, px, rgb};
use gpui_kit::component::Sizable;
use gpui_kit::component::switch::Switch;
use rust_i18n::t;
use rustgrid_config::{
    ConfigStore, DEFAULT_MCP_MAX_ROWS, DEFAULT_MCP_TIMEOUT_SECONDS, MCP_CONFIG_VERSION, McpConfig,
    McpConnectionPermission,
};

use super::AppView;
use super::options::*;
use crate::app::ui::{TextInput, TextInputOptions};

/// Which client configuration the Options page is previewing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum McpClient {
    #[default]
    Claude,
    Cursor,
    Generic,
}

impl McpClient {
    fn label(self) -> String {
        match self {
            McpClient::Claude => t!("options.mcp.claude").to_string(),
            McpClient::Cursor => t!("options.mcp.cursor").to_string(),
            McpClient::Generic => t!("options.mcp.custom").to_string(),
        }
    }

    fn id(self) -> &'static str {
        match self {
            McpClient::Claude => "mcp-client-claude",
            McpClient::Cursor => "mcp-client-cursor",
            McpClient::Generic => "mcp-client-generic",
        }
    }
}

/// Staged MCP settings for the Options window.
///
/// The page is deliberately small: the global switch, the row cap/timeout, the stdio method, the
/// client snippet, and one read-only/full-access switch per saved connection. The server still
/// understands the advanced `allowed_databases`/`blacklisted_tables`/`context_prompt` fields in
/// `mcp.json`; this page preserves them but does not edit them.
#[derive(Debug, Clone)]
pub(super) struct McpOptionsState {
    pub enabled: bool,
    pub max_rows: String,
    pub timeout_seconds: String,
    pub permissions: BTreeMap<String, McpConnectionPermission>,
    client: McpClient,
    copied: Option<String>,
    pub error: Option<String>,
}

impl Default for McpOptionsState {
    fn default() -> Self {
        Self {
            enabled: false,
            max_rows: DEFAULT_MCP_MAX_ROWS.to_string(),
            timeout_seconds: DEFAULT_MCP_TIMEOUT_SECONDS.to_string(),
            permissions: BTreeMap::new(),
            client: McpClient::default(),
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
                client: McpClient::default(),
                copied: None,
                error: None,
            },
            Err(error) => Self {
                error: Some(format!("Could not read mcp.json: {error}")),
                ..Self::default()
            },
        }
    }

    fn permission(&self, profile_id: &str) -> McpConnectionPermission {
        self.permissions
            .get(profile_id)
            .cloned()
            .unwrap_or_default()
    }

    /// The switch is off for the safe default (read-only) and on for the explicit full grant.
    fn is_full_access(&self, profile_id: &str) -> bool {
        !self.permission(profile_id).read_only
    }

    fn toggle_full_access(&mut self, profile_id: &str) {
        let permission = self.permissions.entry(profile_id.to_string()).or_default();
        permission.read_only = !permission.read_only;
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

    /// The AI / MCP page: a handful of global controls, a client snippet and the per-connection
    /// read-only/full-access switch.
    pub(super) fn options_mcp_body(&self, cx: &mut Context<'_, Self>) -> AnyElement {
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
            .child(setting_row(
                "options-mcp-access-method",
                theme,
                t!("options.mcp.access_method").to_string(),
                Some(t!("options.mcp.access_method_hint").to_string()),
                div()
                    .flex_none()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(theme.text))
                    .child("stdio")
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

        let tags = [McpClient::Claude, McpClient::Cursor, McpClient::Generic]
            .into_iter()
            .map(|client| {
                let selected = self.mcp_options.client == client;
                div()
                    .id(SharedString::from(client.id()))
                    .px_3()
                    .py_1()
                    .rounded(px(999.0))
                    .border_1()
                    .border_color(rgb(if selected {
                        theme.primary
                    } else {
                        theme.border
                    }))
                    .bg(rgb(if selected {
                        theme.primary
                    } else {
                        theme.dialog_bg
                    }))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(rgb(if selected { 0xffffff } else { theme.text }))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.mcp_options.client = client;
                        cx.notify();
                    }))
                    .child(client.label())
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let executable = std::env::current_exe()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| "RustGrid".to_string());
        let client_config = match self.mcp_options.client {
            McpClient::Claude | McpClient::Cursor => serde_json::json!({
                "mcpServers": {
                    "rustgrid": {
                        "command": executable,
                        "args": ["--mcp-stdio"]
                    }
                }
            }),
            McpClient::Generic => serde_json::json!({
                "command": executable,
                "args": ["--mcp-stdio"]
            }),
        };
        let client_config =
            serde_json::to_string_pretty(&client_config).unwrap_or_else(|_| "{}".to_string());

        let copy_config = {
            let text = client_config.clone();
            self.dialog_button(
                "mcp-copy-config",
                t!("options.mcp.copy_config").to_string(),
                false,
                cx.listener(move |this, _event, _window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    this.mcp_options.copied = Some(t!("options.mcp.copied").to_string());
                    cx.notify();
                }),
            )
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_wrap()
                    .gap_2()
                    .px_4()
                    .pb_3()
                    .children(tags),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap_2()
                    .px_4()
                    .pb_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .p_3()
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.header_bg))
                            .font_family(SharedString::from("monospace"))
                            .text_size(px(11.0))
                            .line_height(px(15.0))
                            .text_color(rgb(theme.text))
                            .child(client_config.clone()),
                    )
                    .child(copy_config),
            )
            .child(
                div()
                    .px_4()
                    .pb_3()
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
                            .w(px(180.0))
                            .truncate()
                            .child(t!("options.mcp.matrix_name").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_end()
                            .child(t!("options.mcp.matrix_permission").to_string()),
                    )
                    .into_any_element(),
            );

            for (index, node) in self.connections.iter().enumerate() {
                let full_access = self.mcp_options.is_full_access(&node.profile.id);
                let profile_id = node.profile.id.clone();
                let switch_id = SharedString::from(format!("mcp-permission-{profile_id}"));
                let last = index + 1 == self.connections.len();
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
                                .w(px(180.0))
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
                            div()
                                .flex_1()
                                .flex()
                                .flex_row()
                                .items_center()
                                .justify_end()
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "mcp-permission-toggle-{profile_id}"
                                        )))
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            this.mcp_options.toggle_full_access(&profile_id);
                                            cx.notify();
                                        }))
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(if full_access {
                                                    theme.text_muted
                                                } else {
                                                    theme.text
                                                }))
                                                .child(
                                                    t!("options.mcp.matrix_readonly").to_string(),
                                                ),
                                        )
                                        .child(
                                            Switch::new(switch_id)
                                                .checked(full_access)
                                                .with_size(gpui_kit::component::Size::Small)
                                                .tab_stop(false),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.5))
                                                .text_color(rgb(if full_access {
                                                    theme.danger
                                                } else {
                                                    theme.text_muted
                                                }))
                                                .child(t!("options.mcp.full_access").to_string()),
                                        ),
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
}
