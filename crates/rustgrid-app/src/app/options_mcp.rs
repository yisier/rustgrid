use std::rc::Rc;

use gpui::prelude::*;
use gpui::{ClipboardItem, Context, FontWeight, SharedString, div, px, rgb, rgba, svg};
use gpui_kit::component::Sizable;
use gpui_kit::component::switch::Switch;
use rust_i18n::t;

use super::AppView;
use super::options::*;
use crate::app::ui::{TextInput, TextInputOptions};
use crate::session::Loadable;
use crate::theme::Theme;

impl AppView {
    /// Stage the MCP default max rows limit and show it in its text field.
    pub(super) fn set_options_mcp_default_max_rows(
        &mut self,
        value: String,
        cx: &mut Context<'_, Self>,
    ) {
        self.mcp_options.default_max_rows = value.clone();
        if let Some(input) = self.options_mcp_max_rows_input.clone() {
            input.update(cx, |input, cx| input.set_text(value, cx));
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                    let _ = weak.update(cx, |app, cx| {
                        app.mcp_options.default_max_rows = digits;
                        cx.notify();
                    });
                },
            ))
        });
        self.options_mcp_max_rows_input = Some(input);
    }

    /// Stage the MCP HTTP port and show it in its text field.
    pub(super) fn set_options_mcp_http_port(&mut self, value: String, cx: &mut Context<'_, Self>) {
        self.mcp_options.port = value.clone();
        if let Some(input) = self.options_mcp_port_input.clone() {
            input.update(cx, |input, cx| input.set_text(value, cx));
            return;
        }
        let theme = self.theme;
        let weak = cx.weak_entity();
        let input = cx.new(move |cx| {
            TextInput::new(theme, value, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
                    let _ = weak.update(cx, |app, cx| {
                        app.mcp_options.port = digits;
                        cx.notify();
                    });
                },
            ))
        });
        self.options_mcp_port_input = Some(input);
    }

    /// The AI / MCP page: global server switch, default row limit, and connection authorization matrix.
    pub(super) fn options_mcp_body(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = self.theme;

        if self.options_mcp_max_rows_input.is_none() {
            let weak = cx.weak_entity();
            let val = self.mcp_options.default_max_rows.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.set_options_mcp_default_max_rows(val, cx);
                });
            });
        }
        if self.options_mcp_port_input.is_none() {
            let weak = cx.weak_entity();
            let val = self.mcp_options.port.clone();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |this, cx| {
                    this.set_options_mcp_http_port(val, cx);
                });
            });
        }

        let active_port = self.mcp_server.active_port();
        let host = if self.mcp_options.allow_lan {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        let http_status_text = if active_port > 0 {
            t!(
                "options.mcp.http_running",
                host = host,
                port = active_port.to_string()
            )
            .to_string()
        } else {
            t!("options.mcp.http_stopped").to_string()
        };
        let http_status_color = if active_port > 0 {
            theme.primary
        } else {
            theme.text_muted
        };

        let copy_prompt_button = self.dialog_button(
            "options-mcp-copy-prompt",
            t!("options.mcp.copy_agent_prompt").to_string(),
            false,
            cx.listener(move |this, _event, _window, cx| {
                let mcp_cfg = rustgrid_mcp::load_mcp_config(this.config.root());
                let prompt = rustgrid_mcp::generate_agent_prompt(
                    active_port,
                    &mcp_cfg.api_token,
                    this.mcp_options.allow_lan,
                );
                cx.write_to_clipboard(ClipboardItem::new_string(prompt));
                this.mcp_options.copied_toast = Some(t!("options.mcp.prompt_copied").to_string());
                cx.notify();
            }),
        );

        let global_card = settings_card(theme)
            .child(
                toggle_row(
                    theme,
                    "options-mcp-global-enabled",
                    t!("options.mcp.global_enabled").to_string(),
                    Some(t!("options.mcp.global_enabled_hint").to_string()),
                    self.mcp_options.global_enabled,
                    true,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.mcp_options.global_enabled = !this.mcp_options.global_enabled;
                    cx.notify();
                })),
            )
            .child(
                toggle_row(
                    theme,
                    "options-mcp-allow-lan",
                    t!("options.mcp.allow_lan").to_string(),
                    Some(t!("options.mcp.allow_lan_hint").to_string()),
                    self.mcp_options.allow_lan,
                    self.mcp_options.global_enabled,
                )
                .on_click(cx.listener(|this, _event, _window, cx| {
                    if this.mcp_options.global_enabled {
                        this.mcp_options.allow_lan = !this.mcp_options.allow_lan;
                        cx.notify();
                    }
                })),
            )
            .child(setting_row(
                "options-mcp-http-port",
                theme,
                t!("options.mcp.http_port").to_string(),
                Some(t!("options.mcp.http_port_hint").to_string()),
                div()
                    .w(px(100.0))
                    .flex_none()
                    .children(self.options_mcp_port_input.clone())
                    .into_any_element(),
                true,
            ))
            .child(setting_row(
                "options-mcp-default-max-rows",
                theme,
                t!("options.mcp.default_max_rows").to_string(),
                Some(t!("options.mcp.default_max_rows_hint").to_string()),
                div()
                    .w(px(100.0))
                    .flex_none()
                    .children(self.options_mcp_max_rows_input.clone())
                    .into_any_element(),
                true,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(rgb(http_status_color)),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(rgb(http_status_color))
                                    .child(http_status_text),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .when_some(self.mcp_options.copied_toast.as_ref(), |el, toast| {
                                el.child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(theme.primary))
                                        .child(toast.clone()),
                                )
                            })
                            .child(copy_prompt_button),
                    ),
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

            for (idx, node) in self.connections.iter().enumerate() {
                let mcp_settings = rustgrid_mcp::security::McpConnectionSettings::from_options(
                    &node.profile.options,
                );
                let is_last = idx + 1 == self.connections.len();
                let conn_id = node.profile.id.clone();
                let conn_id_mode = node.profile.id.clone();
                let conn_id_edit = node.profile.id.clone();

                let mode_badge = if mcp_settings.enabled {
                    div()
                        .id(SharedString::from(format!("mcp-mode-{}", node.profile.id)))
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.0))
                        .cursor_pointer()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::MEDIUM)
                        .border_1()
                        .bg(rgb(theme.dialog_bg))
                        .border_color(rgb(theme.border))
                        .text_color(rgb(if mcp_settings.read_only {
                            theme.text
                        } else {
                            theme.primary
                        }))
                        .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            if let Some(node) = this
                                .connections
                                .iter_mut()
                                .find(|n| n.profile.id == conn_id_mode)
                            {
                                let mut s =
                                    rustgrid_mcp::security::McpConnectionSettings::from_options(
                                        &node.profile.options,
                                    );
                                s.read_only = !s.read_only;
                                s.write_to_options(&mut node.profile.options);
                                this.persist_profiles(cx);
                                cx.notify();
                            }
                        }))
                        .child(if mcp_settings.read_only {
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

                let scope_element = if !mcp_settings.enabled {
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(theme.text_muted))
                        .child("-")
                        .into_any_element()
                } else {
                    let conn_id_scope = node.profile.id.clone();
                    let display_str = if mcp_settings.allowed_databases.is_empty() {
                        t!("options.mcp.matrix_all_db").to_string()
                    } else if mcp_settings.allowed_databases.len() == 1 {
                        mcp_settings.allowed_databases[0].clone()
                    } else {
                        t!(
                            "options.mcp.matrix_count_db",
                            count = mcp_settings.allowed_databases.len()
                        )
                        .to_string()
                    };
                    div()
                        .id(SharedString::from(format!(
                            "mcp-scope-btn-{}",
                            node.profile.id
                        )))
                        .cursor_pointer()
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.0))
                        .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                        .text_size(px(11.5))
                        .text_color(rgb(if mcp_settings.allowed_databases.is_empty() {
                            theme.text_muted
                        } else {
                            theme.primary
                        }))
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.open_mcp_scope_dialog(&conn_id_scope, cx);
                        }))
                        .child(display_str)
                        .into_any_element()
                };

                matrix_rows.push(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .px_4()
                        .py_2()
                        .when(!is_last, |row| {
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
                            div()
                                .w(px(70.0))
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "mcp-toggle-{}",
                                            node.profile.id
                                        )))
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            if let Some(node) = this
                                                .connections
                                                .iter_mut()
                                                .find(|n| n.profile.id == conn_id)
                                            {
                                                let mut s =
                                                    rustgrid_mcp::security::McpConnectionSettings::from_options(
                                                        &node.profile.options,
                                                    );
                                                s.enabled = !s.enabled;
                                                s.write_to_options(&mut node.profile.options);
                                                this.persist_profiles(cx);
                                                cx.notify();
                                            }
                                        }))
                                        .child(
                                            Switch::new(SharedString::from(format!(
                                                "mcp-sw-{}",
                                                node.profile.id
                                            )))
                                            .checked(mcp_settings.enabled)
                                            .with_size(gpui_kit::component::Size::Small)
                                            .tab_stop(false),
                                        ),
                                 ),
                        )
                        .child(
                            div()
                                .w(px(80.0))
                                .flex()
                                .items_center()
                                .child(mode_badge),
                        )
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .child(scope_element),
                        )
                        .child(
                            div()
                                .w(px(60.0))
                                .flex()
                                .justify_end()
                                .child(
                                    div()
                                        .id(SharedString::from(format!(
                                            "mcp-edit-{}",
                                            node.profile.id
                                        )))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.0))
                                        .cursor_pointer()
                                        .text_size(px(11.5))
                                        .text_color(rgb(theme.primary))
                                        .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                            this.open_mcp_scope_dialog(&conn_id_edit, cx);
                                        }))
                                        .child(t!("options.mcp.matrix_edit").to_string()),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }

        let matrix_card = settings_card(theme).children(matrix_rows);

        let modal = self
            .mcp_options
            .editing_scope
            .as_ref()
            .map(|scope| self.render_mcp_scope_modal(scope, &theme, cx));

        div()
            .relative()
            .flex()
            .flex_col()
            .gap_4()
            .w_full()
            .child(section_label(
                theme,
                t!("options.mcp.global_section").to_string(),
            ))
            .child(global_card)
            .child(section_label(
                theme,
                t!("options.mcp.matrix_section").to_string(),
            ))
            .child(matrix_card)
            .children(modal)
    }

    pub(super) fn open_mcp_scope_dialog(&mut self, conn_id: &str, cx: &mut Context<'_, Self>) {
        let Some(node) = self.connections.iter().find(|n| n.profile.id == conn_id) else {
            return;
        };
        let mcp_settings =
            rustgrid_mcp::security::McpConnectionSettings::from_options(&node.profile.options);

        let mut available_dbs = Vec::new();
        if let Loadable::Loaded(ref dbs) = node.databases {
            for db in dbs {
                if !db.name.trim().is_empty() && !available_dbs.contains(&db.name) {
                    available_dbs.push(db.name.clone());
                }
            }
        }
        if let Some(ref db) = node.profile.database {
            if !db.trim().is_empty() && !available_dbs.contains(db) {
                available_dbs.push(db.clone());
            }
        }
        available_dbs.sort();

        let is_all = mcp_settings.allowed_databases.is_empty();
        let mut selected_dbs = std::collections::BTreeSet::new();
        for db in &mcp_settings.allowed_databases {
            selected_dbs.insert(db.clone());
        }
        let manual_input = if is_all {
            String::new()
        } else {
            mcp_settings.allowed_databases.join(", ")
        };

        let weak = cx.weak_entity();
        let theme = self.theme;
        let init_val = manual_input.clone();
        let input = cx.new(move |cx| {
            TextInput::new(theme, init_val, TextInputOptions::default(), cx).on_change(Rc::new(
                move |text, _window, cx| {
                    let _ = weak.update(cx, |app, cx| {
                        if let Some(ref mut st) = app.mcp_options.editing_scope {
                            st.manual_input = text.to_string();
                        }
                        cx.notify();
                    });
                },
            ))
        });
        self.options_mcp_scope_input = Some(input);

        self.mcp_options.editing_scope = Some(rustgrid_mcp::config::McpDatabaseScopeState {
            conn_id: node.profile.id.clone(),
            conn_name: node.profile.name.clone(),
            driver: node.profile.driver.as_str().to_string(),
            is_all,
            selected_dbs,
            manual_input,
            available_dbs,
        });
        cx.notify();
    }

    fn render_mcp_scope_modal(
        &self,
        scope: &rustgrid_mcp::config::McpDatabaseScopeState,
        theme: &Theme,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let is_all = scope.is_all;
        let conn_name = scope.conn_name.clone();
        let driver = scope.driver.clone();
        let available_dbs = scope.available_dbs.clone();
        let selected_dbs = scope.selected_dbs.clone();

        let all_border = if is_all { theme.primary } else { theme.border };
        let all_bg = if is_all {
            theme.tree_selected_bg
        } else {
            theme.dialog_bg
        };
        let custom_border = if !is_all { theme.primary } else { theme.border };
        let custom_bg = if !is_all {
            theme.tree_selected_bg
        } else {
            theme.dialog_bg
        };

        div()
            .absolute()
            .inset_0()
            .bg(rgba(0x00000080))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(460.0))
                    .max_h(px(540.0))
                    .flex()
                    .flex_col()
                    .bg(rgb(theme.dialog_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .rounded(px(8.0))
                    .shadow_lg()
                    .overflow_hidden()
                    // Header
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .px_4()
                            .py_3()
                            .border_b_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.header_bg))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .text_size(px(13.0))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(theme.text))
                                            .child(t!("options.mcp.scope_modal_title").to_string()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.0))
                                            .text_color(rgb(theme.text_muted))
                                            .child(format!("{conn_name} ({driver})")),
                                    ),
                            )
                            .child(
                                div()
                                    .id("mcp-scope-modal-close")
                                    .cursor_pointer()
                                    .p_1()
                                    .rounded(px(4.0))
                                    .hover(|el| el.bg(rgb(theme.button_hover_bg)))
                                    .text_color(rgb(theme.text_muted))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.mcp_options.editing_scope = None;
                                        this.options_mcp_scope_input = None;
                                        cx.notify();
                                    }))
                                    .child(
                                        svg()
                                            .path("icons/cross.svg")
                                            .w(px(12.0))
                                            .h(px(12.0)),
                                    ),
                            ),
                    )
                    // Content
                    .child(
                        div()
                            .id("mcp-scope-modal-scroll")
                            .flex()
                            .flex_col()
                            .gap_3()
                            .p_4()
                            .overflow_y_scroll()
                            // Option 1: All databases
                            .child(
                                div()
                                    .id("mcp-scope-modal-opt-all")
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .p_3()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(rgb(all_border))
                                    .bg(rgb(all_bg))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        if let Some(ref mut st) = this.mcp_options.editing_scope {
                                            st.is_all = true;
                                            cx.notify();
                                        }
                                    }))
                                    .child(
                                        div()
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(if is_all {
                                                theme.primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(is_all, |dot| {
                                                dot.child(
                                                    div()
                                                        .w(px(8.0))
                                                        .h(px(8.0))
                                                        .rounded_full()
                                                        .bg(rgb(theme.primary)),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(theme.text))
                                                    .child(
                                                        t!("options.mcp.scope_mode_all")
                                                            .to_string(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(
                                                        t!("options.mcp.scope_mode_all_hint")
                                                            .to_string(),
                                                    ),
                                            ),
                                    ),
                            )
                            // Option 2: Specific databases
                            .child(
                                div()
                                    .id("mcp-scope-modal-opt-custom")
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .gap_3()
                                    .p_3()
                                    .rounded(px(6.0))
                                    .border_1()
                                    .border_color(rgb(custom_border))
                                    .bg(rgb(custom_bg))
                                    .cursor_pointer()
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        if let Some(ref mut st) = this.mcp_options.editing_scope {
                                            st.is_all = false;
                                            cx.notify();
                                        }
                                    }))
                                    .child(
                                        div()
                                            .w(px(14.0))
                                            .h(px(14.0))
                                            .rounded_full()
                                            .border_1()
                                            .border_color(rgb(if !is_all {
                                                theme.primary
                                            } else {
                                                theme.text_muted
                                            }))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .when(!is_all, |dot| {
                                                dot.child(
                                                    div()
                                                        .w(px(8.0))
                                                        .h(px(8.0))
                                                        .rounded_full()
                                                        .bg(rgb(theme.primary)),
                                                )
                                            }),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(
                                                div()
                                                    .text_size(px(12.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(rgb(theme.text))
                                                    .child(
                                                        t!("options.mcp.scope_mode_custom")
                                                            .to_string(),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.0))
                                                    .text_color(rgb(theme.text_muted))
                                                    .child(
                                                        t!("options.mcp.scope_mode_custom_hint")
                                                            .to_string(),
                                                    ),
                                            ),
                                    ),
                            )
                            // When specific databases is selected: show database list and manual input
                            .when(!is_all, |col| {
                                col.child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_2()
                                        .p_3()
                                        .rounded(px(6.0))
                                        .border_1()
                                        .border_color(rgb(theme.border))
                                        .bg(rgb(theme.dialog_face))
                                        .when(!available_dbs.is_empty(), |sub| {
                                            sub.child(
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
                                                            .child(
                                                                t!("options.mcp.scope_available_list")
                                                                    .to_string(),
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .flex()
                                                            .flex_row()
                                                            .gap_2()
                                                            .child(
                                                                div()
                                                                    .id("mcp-scope-modal-select-all")
                                                                    .cursor_pointer()
                                                                    .text_size(px(11.0))
                                                                    .text_color(rgb(theme.primary))
                                                                    .hover(|el| el.underline())
                                                                    .on_click(cx.listener(
                                                                        move |this, _event, _window, cx| {
                                                                            if let Some(ref mut st) =
                                                                                this.mcp_options
                                                                                    .editing_scope
                                                                            {
                                                                                for db in &st.available_dbs {
                                                                                    st.selected_dbs
                                                                                        .insert(db.clone());
                                                                                }
                                                                                let joined = st
                                                                                    .selected_dbs
                                                                                    .iter()
                                                                                    .cloned()
                                                                                    .collect::<Vec<_>>()
                                                                                    .join(", ");
                                                                                st.manual_input =
                                                                                    joined.clone();
                                                                                if let Some(ref input) =
                                                                                    this.options_mcp_scope_input
                                                                                {
                                                                                    input.update(
                                                                                        cx,
                                                                                        |inp, cx| {
                                                                                            inp.set_text(
                                                                                                joined,
                                                                                                cx,
                                                                                            )
                                                                                        },
                                                                                    );
                                                                                }
                                                                                cx.notify();
                                                                            }
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        t!("options.mcp.scope_select_all")
                                                                            .to_string(),
                                                                    ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .id("mcp-scope-modal-clear")
                                                                    .cursor_pointer()
                                                                    .text_size(px(11.0))
                                                                    .text_color(rgb(theme.text_muted))
                                                                    .hover(|el| el.underline())
                                                                    .on_click(cx.listener(
                                                                        move |this, _event, _window, cx| {
                                                                            if let Some(ref mut st) =
                                                                                this.mcp_options
                                                                                    .editing_scope
                                                                            {
                                                                                st.selected_dbs.clear();
                                                                                st.manual_input =
                                                                                    String::new();
                                                                                if let Some(ref input) =
                                                                                    this.options_mcp_scope_input
                                                                                {
                                                                                    input.update(
                                                                                        cx,
                                                                                        |inp, cx| {
                                                                                            inp.set_text(
                                                                                                String::new(),
                                                                                                cx,
                                                                                            )
                                                                                        },
                                                                                    );
                                                                                }
                                                                                cx.notify();
                                                                            }
                                                                        },
                                                                    ))
                                                                    .child(
                                                                        t!("options.mcp.scope_clear")
                                                                            .to_string(),
                                                                    ),
                                                            ),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .id("mcp-scope-modal-db-scroll")
                                                    .max_h(px(140.0))
                                                    .overflow_y_scroll()
                                                    .flex()
                                                    .flex_col()
                                                    .gap_1()
                                                    .children(available_dbs.iter().map(|db_name| {
                                                        let checked =
                                                            selected_dbs.contains(db_name);
                                                        let db_click = db_name.clone();
                                                        div()
                                                            .id(SharedString::from(format!("mcp-scope-item-{}", db_name)))
                                                            .flex()
                                                            .flex_row()
                                                            .items_center()
                                                            .gap_2()
                                                            .px_2()
                                                            .py_1()
                                                            .rounded(px(4.0))
                                                            .cursor_pointer()
                                                            .hover(|el| {
                                                                el.bg(rgb(theme.button_hover_bg))
                                                            })
                                                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                                                if let Some(ref mut st) =
                                                                    this.mcp_options.editing_scope
                                                                {
                                                                    if st.selected_dbs.contains(&db_click) {
                                                                        st.selected_dbs.remove(&db_click);
                                                                    } else {
                                                                        st.selected_dbs.insert(db_click.clone());
                                                                    }
                                                                    let joined = st
                                                                        .selected_dbs
                                                                        .iter()
                                                                        .cloned()
                                                                        .collect::<Vec<_>>()
                                                                        .join(", ");
                                                                    st.manual_input = joined.clone();
                                                                    if let Some(ref input) =
                                                                        this.options_mcp_scope_input
                                                                    {
                                                                        input.update(
                                                                            cx,
                                                                            |inp, cx| {
                                                                                inp.set_text(
                                                                                    joined, cx,
                                                                                )
                                                                            },
                                                                        );
                                                                    }
                                                                    cx.notify();
                                                                }
                                                            }))
                                                            .child(
                                                                div()
                                                                    .w(px(14.0))
                                                                    .h(px(14.0))
                                                                    .rounded(px(3.0))
                                                                    .border_1()
                                                                    .border_color(rgb(if checked {
                                                                        theme.primary
                                                                    } else {
                                                                        theme.border
                                                                    }))
                                                                    .bg(rgb(if checked {
                                                                        theme.primary
                                                                    } else {
                                                                        theme.dialog_bg
                                                                    }))
                                                                    .flex()
                                                                    .items_center()
                                                                    .justify_center()
                                                                    .when(checked, |chk| {
                                                                        chk.child(
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
                                                                    .child(db_name.clone()),
                                                            )
                                                    })),
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
                                                        .child(
                                                            t!("options.mcp.scope_manual_label")
                                                                .to_string(),
                                                        ),
                                                )
                                                .children(self.options_mcp_scope_input.clone()),
                                        ),
                                )
                            }),
                    )
                    // Footer
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .border_t_1()
                            .border_color(rgb(theme.border))
                            .bg(rgb(theme.header_bg))
                            .child(self.dialog_button(
                                "mcp-scope-cancel",
                                t!("options.mcp.scope_cancel").to_string(),
                                false,
                                cx.listener(|this, _event, _window, cx| {
                                    this.mcp_options.editing_scope = None;
                                    this.options_mcp_scope_input = None;
                                    cx.notify();
                                }),
                            ))
                            .child(self.dialog_button(
                                "mcp-scope-save",
                                t!("options.mcp.scope_save").to_string(),
                                true,
                                cx.listener(move |this, _event, _window, cx| {
                                    if let Some(st) = this.mcp_options.editing_scope.take() {
                                        this.options_mcp_scope_input = None;
                                        if let Some(node) = this
                                            .connections
                                            .iter_mut()
                                            .find(|n| n.profile.id == st.conn_id)
                                        {
                                            let mut s =
                                                rustgrid_mcp::security::McpConnectionSettings::from_options(
                                                    &node.profile.options,
                                                );
                                            if st.is_all {
                                                s.allowed_databases.clear();
                                            } else {
                                                let mut dbs = std::collections::BTreeSet::new();
                                                for db in st.selected_dbs {
                                                    dbs.insert(db);
                                                }
                                                for part in st.manual_input.split(',') {
                                                    let trimmed = part.trim();
                                                    if !trimmed.is_empty() {
                                                        dbs.insert(trimmed.to_string());
                                                    }
                                                }
                                                s.allowed_databases = dbs.into_iter().collect();
                                            }
                                            s.write_to_options(&mut node.profile.options);
                                            this.persist_profiles(cx);

                                            let mut mcp_cfg =
                                                rustgrid_mcp::config::load_mcp_config(
                                                    this.config.root(),
                                                );
                                            let perm = mcp_cfg
                                                .connections
                                                .entry(st.conn_id.clone())
                                                .or_default();
                                            perm.enabled = s.enabled;
                                            perm.read_only = s.read_only;
                                            perm.allowed_databases = s.allowed_databases.clone();
                                            let _ = rustgrid_mcp::config::save_mcp_config(
                                                this.config.root(),
                                                &mcp_cfg,
                                            );

                                            cx.notify();
                                        }
                                    }
                                }),
                            )),
                    ),
            )
    }
}
