use super::query::query_file_key;
use super::*;
use gpui_kit::component::spinner::Spinner;

impl AppView {
    pub(super) fn render_query_view(
        &mut self,
        query_index: usize,
        _window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let (is_routine, is_view) = match self.queries.get(query_index) {
            Some(query) => (query.routine.is_some(), query.view.is_some()),
            None => return div().into_any_element(),
        };
        if is_routine {
            return self.render_routine_view(query_index, cx);
        }
        if is_view {
            return self.render_view_view(query_index, cx);
        }
        self.sync_query_selection(query_index, cx);
        let theme = self.theme;
        let is_batch = self
            .queries
            .get(query_index)
            .is_some_and(|query| query.batch.is_some());
        // 导出结果 is only useful when the last run produced at least one result set.
        let has_result_sets = self
            .queries
            .get(query_index)
            .is_some_and(|query| query.has_result_grid());

        let key_value = !is_batch
            && self
                .queries
                .get(query_index)
                .and_then(|query| query.connection_index)
                .is_some_and(|index| self.driver_supports(index, DriverCapability::KeyValue));
        let documents = !is_batch
            && self
                .queries
                .get(query_index)
                .and_then(|query| query.connection_index)
                .is_some_and(|index| self.driver_supports(index, DriverCapability::Documents));

        let mut toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_0p5()
            .px_1()
            .py_0p5()
            .flex_none()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(self.query_tool_button(
                "query-save",
                "icons/save.svg",
                t!("query.save").to_string(),
                theme.text,
                theme.text,
                true,
                cx.listener(|this, _event, window, cx| this.begin_save_query(window, cx)),
            ));
        // Redis commands and MongoDB JSON commands are not SQL: there is nothing for 美化SQL to
        // format.
        if !key_value && !documents {
            toolbar = toolbar
                .child(toolbar_separator(theme))
                .child(self.query_tool_button(
                    "query-format",
                    "icons/format_sql.svg",
                    t!("query.format").to_string(),
                    theme.text,
                    theme.text,
                    true,
                    cx.listener(|this, _event, window, cx| this.format_query(window, cx)),
                ));
        }
        toolbar = toolbar
            .child(toolbar_separator(theme))
            .child(self.query_tool_button(
                "query-export",
                "icons/export.svg",
                t!("query.export_results").to_string(),
                theme.text,
                theme.icon_views,
                has_result_sets,
                cx.listener(move |this, _event, _window, cx| {
                    this.open_query_export_choice(query_index, cx)
                }),
            ));

        // Copy the few scalars needed from the tab, so the `queries` borrow ends before the editor
        // (which needs `&mut self`) and the result-panel renderers run.
        let (
            has_connection,
            running,
            has_selection,
            grid_id,
            has_error,
            has_results,
            active_result,
            supports_schemas,
        ) = {
            let Some(query) = self.queries.get(query_index) else {
                return div().into_any_element();
            };
            let is_group = query.batch.is_some();
            (
                // A group query may connect its members on Run, so it is runnable without an
                // already-open connection (an empty group still reports itself in 信息).
                is_group
                    || query
                        .connection_index
                        .and_then(|index| self.connection_arc(index))
                        .is_some(),
                query.running,
                query.caret != query.anchor,
                query.grid_id,
                query.result_error.is_some(),
                if is_group {
                    query.running
                        || query.last_elapsed.is_some()
                        || query.result_error.is_some()
                        || query.batch.as_ref().is_some_and(|batch| {
                            batch
                                .targets
                                .iter()
                                .any(|target| !target.results.is_empty() || target.error.is_some())
                        })
                } else {
                    !query.results.is_empty()
                },
                query.active_result,
                !is_group
                    && query.connection_index.is_some_and(|index| {
                        self.driver_supports(index, DriverCapability::Schemas)
                    }),
            )
        };
        let run_enabled = has_connection && !running;
        // One run button: it runs the selection when there is one, otherwise the whole editor.
        let run_label = if has_selection {
            t!("query.run_selected").to_string()
        } else {
            t!("query.run").to_string()
        };
        // The shortcut it fires is shown on hover — the configured one, since the Options window's
        // 快捷键 page can change (or clear) it.
        let run_tooltip = match self.command_shortcut_label("query.run", cx.platform_info().os) {
            Some(shortcut) => format!("{} ({shortcut})", t!("query.run")),
            None => t!("query.run").to_string(),
        };

        let controls = if is_batch {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .py_0p5()
                .flex_none()
                .bg(rgb(theme.toolbar_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .child(self.group_query_target_element(query_index))
        } else {
            let controls = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .py_0p5()
                .flex_none()
                .bg(rgb(theme.toolbar_bg))
                .border_b_1()
                .border_color(rgb(theme.border))
                .child(self.query_connection_combo_element())
                .child(self.query_database_combo_element());
            if supports_schemas {
                controls.child(self.query_schema_combo_element())
            } else {
                controls
            }
        };
        let controls = controls
            .child(div().w(px(10.0)).flex_none())
            .child(
                self.query_tool_button(
                    "query-run",
                    "icons/run.svg",
                    run_label,
                    theme.text,
                    theme.icon_connection,
                    run_enabled,
                    // The wrapped editor owns the real selection, so `run_query` re-reads it live
                    // and runs the selection when there is one, otherwise the whole editor.
                    cx.listener(move |this, _event, _window, cx| {
                        this.run_query(RunScope::SelectionOrAll, cx)
                    }),
                )
                .tooltip(ui::text_tooltip(run_tooltip)),
            )
            .child(self.query_tool_button(
                "query-stop",
                "icons/stop.svg",
                t!("query.stop").to_string(),
                if running {
                    theme.text
                } else {
                    theme.text_muted
                },
                // Red only while a query runs; grey (and dimmed) otherwise.
                if running {
                    theme.danger
                } else {
                    theme.text_muted
                },
                running,
                cx.listener(|this, _event, _window, cx| this.stop_query(cx)),
            ))
            .child(self.query_timer(query_index, cx));

        let editor = self.render_query_editor(query_index, cx);
        let result_grid = grid_id.and_then(|id| {
            self.grids
                .iter()
                .find(|grid| grid.read(cx).state.id == id)
                .cloned()
        });
        let has_result_panel = running || has_error || has_results || result_grid.is_some();
        let body: AnyElement = if !has_result_panel {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .child(editor)
                .into_any_element()
        } else {
            // `active_result` is 0 for the 信息 tab, 1..=n for the n-th result set (or the n-th
            // connection page of a group query).
            let result_body: AnyElement = if is_batch {
                // A group run keeps every connection's state in its own page and updates it live;
                // there is no single "running" placeholder covering them all.
                self.render_group_result(query_index, cx)
            } else if running {
                div()
                    .flex_1()
                    .p_2()
                    .text_color(rgb(theme.text_muted))
                    .child(t!("query.running").to_string())
                    .into_any_element()
            } else if active_result == 0 || has_error {
                self.render_query_info(query_index, cx)
            } else if let Some(grid) = result_grid {
                grid.into_any_element()
            } else {
                div().into_any_element()
            };
            // The result panel keeps a user-draggable pixel height (Navicat's bottom pane); the
            // editor takes everything above it and never spills over the splitter.
            let mut panel = div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(self.query_result_height))
                .min_h(px(0.0))
                .overflow_hidden()
                .bg(rgb(theme.editor_bg));
            if has_results {
                panel = panel.child(self.render_query_result_tabs(query_index, cx));
            }
            panel = panel.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(result_body),
            );
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h(px(QUERY_EDITOR_MIN_HEIGHT))
                        .overflow_hidden()
                        .child(editor),
                )
                .child(
                    ui::pane_resize_divider_h("query-result-divider", theme).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                            this.begin_query_split_drag(event.position.y, cx);
                        }),
                    ),
                )
                .child(panel)
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(controls)
            .child(body)
            .into_any_element()
    }

    /// The bottom result tabs: a fixed 信息 tab then one 结果 tag per result set. The 结果 tags scroll
    /// horizontally and, only when they do not all fit, ◀ / ▶ buttons page them — the same
    /// pattern as the main tab bar above the pane.
    fn render_query_result_tabs(
        &self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let Some(query) = self.queries.get(query_index) else {
            return div().into_any_element();
        };
        let theme = self.theme;
        let scroll = query.result_tabs.scroll.clone();
        let show_arrows = query.result_tabs.overflow.get();
        let info_tab = self.query_result_tab(
            "query-result-info",
            t!("query.info_tab").to_string(),
            0,
            query.active_result == 0,
            cx,
        );

        let mut strip = div()
            .id("query-result-tab-strip")
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_1()
            .min_w(px(0.0))
            .overflow_x_scroll()
            .track_scroll(&scroll);
        if let Some(batch) = &query.batch {
            // One tag per connection: a failed connection (or a missing password) has a page to
            // report itself in, so every target gets a tag whether or not it produced a grid.
            for (index, target) in batch.targets.iter().enumerate() {
                strip = strip.child(self.group_query_result_tab(
                    query_index,
                    index,
                    target,
                    query.active_result == index + 1,
                    cx,
                ));
            }
        } else {
            let mut result_number = 0usize;
            for (index, _) in query.results.iter().enumerate() {
                if query.result_grids.get(index).copied().flatten().is_none() {
                    continue;
                }
                result_number += 1;
                strip = strip.child(self.query_result_tab(
                    SharedString::from(format!("query-result-{index}")),
                    t!("query.result_tab", n = result_number).to_string(),
                    result_number,
                    query.active_result == result_number,
                    cx,
                ));
            }
        }

        // The strip's content width is only known after layout, so a measurement canvas records
        // the overflow state and asks for one more frame when it changes; the buttons are drawn
        // from the previous measurement.
        let measure_scroll = scroll.clone();
        let overflow = query.result_tabs.overflow.clone();
        let measure = canvas(
            |_bounds, _window: &mut Window, _cx: &mut App| {},
            move |_bounds, _state, window: &mut Window, _cx: &mut App| {
                let overflows = measure_scroll.max_offset().x > px(0.0);
                if overflow.get() != overflows {
                    overflow.set(overflows);
                    window.request_animation_frame();
                }
            },
        );

        let mut bar = div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .h(px(26.0))
            .flex_none()
            .px_2()
            .bg(rgb(theme.toolbar_bg))
            .border_b_1()
            .border_color(rgb(theme.border))
            .child(info_tab);
        if show_arrows {
            bar = bar.child(self.query_result_scroll_button(query_index, true, cx));
        }
        bar = bar.child(strip);
        if show_arrows {
            bar = bar.child(self.query_result_scroll_button(query_index, false, cx));
        }
        bar.child(measure.absolute().inset_0()).into_any_element()
    }

    /// One ◀ / ▶ button of the result tab strip: it pans the tags by about a page.
    fn query_result_scroll_button(
        &self,
        query_index: usize,
        left: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let scroll = self
            .queries
            .get(query_index)
            .map(|query| query.result_tabs.scroll.clone());
        let icon = if left {
            "icons/tab-prev.svg"
        } else {
            "icons/tab-next.svg"
        };
        div()
            .id(SharedString::from(format!(
                "query-result-scroll-{}-{query_index}",
                if left { "left" } else { "right" }
            )))
            .flex()
            .items_center()
            .justify_center()
            .w(px(18.0))
            .h(px(18.0))
            .flex_none()
            .rounded(px(4.0))
            .cursor_pointer()
            .text_color(rgb(theme.text_muted))
            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                let Some(scroll) = scroll.clone() else {
                    return;
                };
                if this.query_result_scroll(&scroll, if left { -180.0 } else { 180.0 }) {
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .path(icon)
                    .w(px(12.0))
                    .h(px(12.0))
                    .flex_none()
                    .text_color(rgb(theme.text_muted)),
            )
            .into_any_element()
    }

    /// Pan the result tab strip by `delta` px. Returns whether it moved.
    fn query_result_scroll(&self, scroll: &ScrollHandle, delta: f32) -> bool {
        let max = f32::from(scroll.max_offset().x);
        if max <= 0.0 {
            return false;
        }
        let current = -f32::from(scroll.offset().x);
        let next = (current + delta).clamp(0.0, max);
        if next == current {
            return false;
        }
        let y = scroll.offset().y;
        scroll.set_offset(Point::new(px(-next), y));
        true
    }

    /// One bottom result tag. `tab` is the value [`AppView::select_query_result`] selects.
    fn query_result_tab(
        &self,
        id: impl Into<SharedString>,
        label: String,
        tab: usize,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        div()
            .id(id.into())
            .flex()
            .items_center()
            .justify_center()
            .h(px(20.0))
            .px_3()
            .rounded(px(4.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_query_result(tab, cx);
            }))
            .child(label)
            .into_any_element()
    }

    /// The group chip shown where a single connection's picker would be. It names the group and
    /// its target count; the actual member list is resolved again on every Run.
    fn group_query_target_element(&self, query_index: usize) -> AnyElement {
        let theme = self.theme;
        let Some(batch) = self
            .queries
            .get(query_index)
            .and_then(|query| query.batch.as_ref())
        else {
            return div().into_any_element();
        };
        let group = groups::find_group(&self.tree_layout, batch.group_id);
        let name = group
            .map(|group| group.name.clone())
            .unwrap_or_else(|| batch.group_name.clone());
        // Resolve the group now (Run does the same): the chip reports the current membership, not
        // the membership of the last run.
        let live_targets = self.resolve_group_targets(batch.group_id);
        let count = live_targets.len();
        let label = t!("query.batch.target_summary", name = name, count = count).to_string();
        let names = live_targets
            .iter()
            .map(|target| target.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let tooltip = if names.is_empty() {
            t!("query.batch.no_targets").to_string()
        } else {
            names
        };
        div()
            .id(SharedString::from(format!(
                "query-group-target-{query_index}"
            )))
            .flex()
            .items_center()
            .gap_1()
            .h(px(20.0))
            .px_2()
            .max_w(px(320.0))
            .rounded(px(4.0))
            .bg(rgb(theme.button_bg))
            .border_1()
            .border_color(rgb(theme.border))
            .text_size(px(12.0))
            .text_color(rgb(theme.text))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .w(px(13.0))
                    .h(px(13.0))
                    .flex_none()
                    .text_color(rgb(theme.icon_queries)),
            )
            .child(div().min_w(px(0.0)).truncate().child(label))
            .tooltip(ui::text_tooltip(tooltip))
            .into_any_element()
    }

    /// One group-query result tag: the connection's name, a status dot (red for a failure or a
    /// missing password, brand while connecting/running), and a tooltip with the state.
    fn group_query_result_tab(
        &self,
        query_index: usize,
        target_index: usize,
        target: &BatchTarget,
        active: bool,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let dot = if target.has_error() {
            Some(theme.danger)
        } else {
            match target.state {
                BatchTargetState::Connecting | BatchTargetState::Running => Some(theme.brand),
                _ => None,
            }
        };
        let tooltip = format!(
            "{} · {}",
            target.name,
            super::query_editor::batch_state_label(
                if target.state == BatchTargetState::Done && target.has_error() {
                    BatchTargetState::Failed
                } else {
                    target.state
                }
            )
        );
        div()
            .id(SharedString::from(format!(
                "query-result-target-{query_index}-{target_index}"
            )))
            .flex()
            .items_center()
            .gap_1()
            .h(px(20.0))
            .px_2()
            .max_w(px(200.0))
            .rounded(px(4.0))
            .text_size(px(11.5))
            .cursor_pointer()
            .when(active, move |style| {
                style
                    .bg(rgb(theme.editor_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text))
            })
            .when(!active, move |style| {
                style
                    .bg(rgb(theme.button_bg))
                    .border_1()
                    .border_color(rgb(theme.border))
                    .text_color(rgb(theme.text_muted))
                    .hover(move |style| style.text_color(rgb(theme.text)))
            })
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_query_result(target_index + 1, cx);
            }))
            .children(dot.map(|color| {
                div()
                    .w(px(6.0))
                    .h(px(6.0))
                    .flex_none()
                    .rounded_full()
                    .bg(rgb(color))
            }))
            .child(div().min_w(px(0.0)).truncate().child(target.name.clone()))
            .tooltip(ui::text_tooltip(tooltip))
            .into_any_element()
    }

    /// The body of the bottom panel for a group query: the 信息 page, or the active connection's
    /// page. A group tab always keeps one page per connection, even when the run failed before
    /// producing a grid.
    fn render_group_result(
        &mut self,
        query_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let active = self
            .queries
            .get(query_index)
            .map(|query| query.active_result)
            .unwrap_or(0);
        if active == 0 {
            return self.render_query_info(query_index, cx);
        }
        self.render_group_target_page(query_index, active - 1, cx)
    }

    /// One connection's result page: its state while running, or its statement results/error.
    fn render_group_target_page(
        &mut self,
        query_index: usize,
        target_index: usize,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let Some(query) = self.queries.get(query_index) else {
            return div().into_any_element();
        };
        let tab_id = query.id;
        let Some(target) = query
            .batch
            .as_ref()
            .and_then(|batch| batch.targets.get(target_index))
        else {
            return div().into_any_element();
        };
        let name = target.name.clone();
        let state = target.state;
        let error = target.error.clone();
        let skipped = target.skipped;

        match state {
            BatchTargetState::Pending => {
                return self.group_status_page(t!("query.batch.state_pending").to_string(), false);
            }
            BatchTargetState::Connecting => {
                return self
                    .group_status_page(t!("query.batch.state_connecting").to_string(), true);
            }
            BatchTargetState::Running => {
                return self.group_status_page(t!("query.batch.state_running").to_string(), true);
            }
            BatchTargetState::Stopped => {
                return self.group_status_page(t!("query.batch.state_stopped").to_string(), false);
            }
            BatchTargetState::Failed | BatchTargetState::NeedsPassword => {
                let title = super::query_editor::batch_state_label(state);
                let mut detail = format!(
                    "{name}\n\n{}",
                    error.unwrap_or_else(|| t!("query.batch.no_error_detail").to_string())
                );
                if state == BatchTargetState::NeedsPassword {
                    detail.push_str("\n\n");
                    detail.push_str(t!("query.batch.needs_password_hint").as_ref());
                }
                return self.render_group_error_page(tab_id, title, detail, cx);
            }
            BatchTargetState::Done => {}
        }

        // The connection is done: extract the small per-statement metadata, then release the
        // `queries` borrow so the error editor/grid renderers can use `&mut self`.
        let (results_len, active_result, statements) = {
            let Some(target) = self
                .queries
                .get(query_index)
                .and_then(|query| query.batch.as_ref())
                .and_then(|batch| batch.targets.get(target_index))
            else {
                return div().into_any_element();
            };
            (
                target.results.len(),
                target.active_result,
                target
                    .results
                    .iter()
                    .map(|result| {
                        (
                            result.statement.clone(),
                            result.error.clone(),
                            result.grid_id,
                            result.has_result_set,
                            result.row_count,
                            result.rows_affected,
                            result.elapsed,
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        };
        if results_len == 0 {
            return self.group_status_page(t!("query.batch.no_results").to_string(), false);
        }
        let active_result = active_result.min(results_len - 1);

        let mut page = div().flex().flex_col().flex_1().min_h(px(0.0));
        if results_len > 1 {
            let mut tabs = div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .flex_none()
                .bg(rgb(theme.toolbar_bg))
                .border_b_1()
                .border_color(rgb(theme.border));
            for (index, (_, error, _, _, _, _, _)) in statements.iter().enumerate() {
                let active = index == active_result;
                let dot = error.as_ref().map(|_| theme.danger);
                tabs = tabs.child(
                    div()
                        .id(SharedString::from(format!(
                            "group-statement-{query_index}-{target_index}-{index}"
                        )))
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(20.0))
                        .px_2()
                        .rounded(px(4.0))
                        .text_size(px(11.5))
                        .cursor_pointer()
                        .when(active, move |style| {
                            style
                                .bg(rgb(theme.editor_bg))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .text_color(rgb(theme.text))
                        })
                        .when(!active, move |style| {
                            style
                                .bg(rgb(theme.button_bg))
                                .border_1()
                                .border_color(rgb(theme.border))
                                .text_color(rgb(theme.text_muted))
                        })
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.select_group_statement(query_index, target_index, index, cx);
                        }))
                        .children(dot.map(|color| {
                            div()
                                .w(px(6.0))
                                .h(px(6.0))
                                .flex_none()
                                .rounded_full()
                                .bg(rgb(color))
                        }))
                        .child(t!("query.batch.statement_tab", n = index + 1).to_string()),
                );
            }
            page = page.child(tabs);
        }
        if skipped > 0 {
            page = page.child(
                div()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .text_size(px(11.5))
                    .text_color(rgb(theme.warning))
                    .child(t!("query.batch.skipped", count = skipped).to_string()),
            );
        }
        let Some((
            statement,
            statement_error,
            grid_id,
            has_result_set,
            row_count,
            rows_affected,
            result_elapsed,
        )) = statements.get(active_result)
        else {
            return div().into_any_element();
        };
        if let Some(error) = statement_error {
            let detail = format!(
                "{} · {name}\n\n{statement}\n\n{error}",
                t!("query.batch.statement_error")
            );
            page.child(self.render_group_error_page(
                tab_id,
                t!("query.batch.statement_error").to_string(),
                detail,
                cx,
            ))
            .into_any_element()
        } else if let Some(grid_id) = grid_id {
            if let Some(grid) = self
                .grids
                .iter()
                .find(|grid| grid.read(cx).state.id == *grid_id)
                .cloned()
            {
                page.child(grid).into_any_element()
            } else {
                div().into_any_element()
            }
        } else {
            let outcome = if *has_result_set {
                t!("query.result_rows", count = row_count).to_string()
            } else if *rows_affected > 0 {
                t!("query.rows_affected", count = rows_affected).to_string()
            } else {
                t!("query.executed").to_string()
            };
            let line = format!(
                "> {outcome}  ·  {} {}",
                t!("query.elapsed"),
                super::query_editor::format_elapsed(*result_elapsed)
            );
            page.child(
                div()
                    .flex()
                    .flex_1()
                    .p_3()
                    .text_size(px(12.0))
                    .text_color(rgb(theme.text_muted))
                    .child(line),
            )
            .into_any_element()
        }
    }

    /// A centered status page (pending / connecting / running / stopped).
    fn group_status_page(&self, label: String, spinning: bool) -> AnyElement {
        let theme = self.theme;
        let mut row = div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .py_4()
            .text_size(px(12.0))
            .text_color(rgb(theme.text_muted));
        if spinning {
            row = row.child(Spinner::new().color(rgb(theme.text_muted).into()));
        }
        row.child(label).into_any_element()
    }

    /// A connection/statement error, with a read-only selectable/copyable editor (the same rule as
    /// the 信息 log: text the user must copy goes through `ui::SqlEditor`).
    fn render_group_error_page(
        &mut self,
        tab_id: u64,
        title: String,
        detail: String,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let editor = self
            .batch_error_editors
            .entry(tab_id)
            .or_insert_with(|| {
                cx.new(|cx| {
                    ui::SqlEditor::new(cx).options(ui::SqlEditorOptions {
                        language: "text".into(),
                        font_size: 12.0,
                        line_number: false,
                        readonly: true,
                        ..Default::default()
                    })
                })
            })
            .clone();
        if editor.read(cx).text() != detail {
            editor.update(cx, |editor, cx| editor.set_text(detail.clone(), cx));
        }
        let copy = detail.clone();
        div()
            .id(SharedString::from(format!("group-error-{tab_id}")))
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(theme.border))
                    .child(
                        svg()
                            .path("icons/warning.svg")
                            .w(px(14.0))
                            .h(px(14.0))
                            .flex_none()
                            .text_color(rgb(theme.danger)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(12.0))
                            .text_color(rgb(theme.danger))
                            .child(title),
                    )
                    .child(
                        div()
                            .id("group-error-copy")
                            .flex_none()
                            .h(px(20.0))
                            .px_2()
                            .rounded(px(4.0))
                            .text_size(px(11.5))
                            .text_color(rgb(theme.text_muted))
                            .cursor_pointer()
                            .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
                            .on_click(cx.listener(move |_this, _event, _window, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    copy.clone(),
                                ));
                            }))
                            .child(t!("query.batch.copy_error").to_string()),
                    ),
            )
            .child(div().flex().flex_1().min_h(px(0.0)).child(editor))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn query_tool_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: String,
        color: u32,
        icon_color: u32,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let theme = self.theme;
        div()
            .id(id)
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(24.0))
            .rounded_sm()
            .text_size(px(12.0))
            .text_color(rgb(color))
            .when(enabled, move |style| {
                style
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgb(theme.tree_hover_bg)))
            })
            .when(enabled, |this| this.on_click(on_click))
            // A disabled button (Stop while nothing runs, Run while running) looks disabled.
            .when(!enabled, |this| this.opacity(0.4))
            .child(
                svg()
                    .path(icon)
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex_none()
                    .text_color(rgb(icon_color)),
            )
            .child(label)
    }

    fn query_connection_combo_element(&self) -> AnyElement {
        match self.query_connection_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn query_database_combo_element(&self) -> AnyElement {
        match self.query_database_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    fn query_schema_combo_element(&self) -> AnyElement {
        match self.query_schema_combo.as_ref() {
            Some(combo) => combo.clone().into_any_element(),
            None => div().into_any_element(),
        }
    }

    /// The saved-query list shown under the Queries main tab. Like the Backup tab, it is scoped to
    /// the selected (opened) database and behaves like a folder of `.sql` files: double-click opens,
    /// right-click (or F2 / Ctrl+C / Ctrl+V) manages the file. The list offers the shared
    /// 详细列表 / 平铺网格 layouts.
    pub(super) fn render_saved_queries(&self, cx: &mut Context<'_, Self>) -> AnyElement {
        let theme = self.theme;
        let mut container = div()
            .id("saved-query-list")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_hidden()
            .track_focus(&self.query_list_focus)
            .key_context(QUERY_LIST_CONTEXT)
            .on_action(cx.listener(|this, _: &QueryFocusSearch, window, cx| {
                this.focus_object_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &RenameQueryFile, window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.begin_rename_query(index, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &CopyQueryFile, _window, cx| {
                if let Some(index) = this.saved_query_selected {
                    this.copy_query_file(index);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &PasteQueryFile, _window, cx| {
                this.paste_query_file(cx);
            }))
            .on_action(cx.listener(|this, _: &SelectAllQueries, _window, cx| {
                this.select_all_queries(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    // A click on the list background is "outside" the in-place rename editor, so it
                    // commits the rename (clicking a non-focusable area does not move focus, so the
                    // blur subscription alone would not end the edit).
                    if this.query_rename.is_some() {
                        this.submit_query_rename(cx);
                        window.focus(&this.query_list_focus, cx);
                        return;
                    }
                    window.focus(&this.query_list_focus, cx);
                    this.begin_marquee(MarqueeTarget::Queries, event.position, event.modifiers, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                    this.context_menu = Some(ContextMenu {
                        target: ContextTarget::QueryList,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            // The 平铺 grid draws its own horizontal scrollbar; these keep its drag alive anywhere
            // in the window while the pointer leaves the 14px track.
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if this.query_detail_columns.borrow_mut().drag_resize(event) {
                    cx.notify();
                }
                this.query_grid_drag(event, cx);
                this.drag_marquee(MarqueeTarget::Queries, event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.query_detail_columns.borrow_mut().end_resize() {
                        cx.notify();
                    }
                    this.query_grid_end(cx);
                    this.end_marquee(cx);
                }),
            );

        if self.query_scope(cx).is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h(px(0.0))
                .into_any_element();
        }

        let visible =
            self.sorted_visible_query_files(cx, &self.object_search_text(VIEW_PAGE_QUERIES));
        let body = match self.view_mode(VIEW_PAGE_QUERIES) {
            ViewMode::Detail => self.render_query_detail(&visible, theme, cx),
            ViewMode::Grid => self.render_query_tiles(&visible, theme, cx),
        };
        container = container.child(body);
        if let Some(rect) = self.marquee_rect_for(MarqueeTarget::Queries) {
            container = container.child(rect);
        }
        container.into_any_element()
    }

    /// The content-fitted widths of the Queries 详细列表 columns (名称 / 修改日期 / 文件大小).
    fn query_detail_widths(&self, visible: &[usize]) -> Vec<f32> {
        let mut longest = [
            ui::approx_text_width(&t!("common.name")) + 64.0,
            ui::approx_text_width(&t!("backup.field.modified")),
            ui::approx_text_width(&t!("backup.field.size")),
        ];
        for index in visible {
            let Some(file) = self.query_files.get(*index) else {
                continue;
            };
            longest[0] = longest[0].max(ui::approx_text_width(&file.name) + 64.0);
            longest[1] = longest[1].max(ui::approx_text_width(&query_modified_text(file)));
            longest[2] = longest[2].max(ui::approx_text_width(&human_size(file.size)));
        }
        vec![
            ui::detail_column_width(longest[0], 220.0),
            ui::detail_column_width(longest[1], 140.0),
            ui::detail_column_width(longest[2], 80.0),
        ]
    }

    /// The Queries 详细列表: a sortable table of the in-scope saved queries.
    fn render_query_detail(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let sort = self.query_sort;
        let fitted = self.query_detail_widths(visible);
        let widths = self.query_detail_columns.borrow_mut().resolve(&fitted);
        let mut header = ui::detail_header_row(theme);
        for (index, (id, label, column)) in [
            (
                "query-sort-name",
                t!("common.name").to_string(),
                QuerySortColumn::Name,
            ),
            (
                "query-sort-modified",
                t!("backup.field.modified").to_string(),
                QuerySortColumn::Modified,
            ),
            (
                "query-sort-size",
                t!("backup.field.size").to_string(),
                QuerySortColumn::Size,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let width = widths[index];
            let cell = ui::detail_header_cell(
                id,
                label,
                (sort.column == column).then_some(sort.descending),
                theme,
                cx.listener(move |this, _event, _window, cx| this.toggle_query_sort(column, cx)),
            );
            header = header.child(ui::detail_header_column(
                SharedString::from(format!("query-resize-{index}")),
                width,
                self.query_detail_columns.borrow().resizing(index),
                theme,
                cell,
                cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    this.query_detail_columns.borrow_mut().begin_resize(
                        index,
                        event.position.x,
                        width,
                    );
                    cx.notify();
                }),
            ));
        }

        let mut list = ui::DetailList::new(
            "query-detail",
            &self.query_detail_scroll,
            ui::detail_content_width(&widths),
            header,
        );
        for &index in visible {
            list = list.child(self.query_detail_row(index, &widths, theme, cx));
        }
        list.render(theme)
    }

    /// One 详细列表 row of a saved query.
    fn query_detail_row(
        &self,
        index: usize,
        widths: &[f32],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let file = &self.query_files[index];
        let key = query_file_key(file);
        let selected = self.query_selection.contains(&key);
        let name: AnyElement = match self.query_rename_input(index) {
            Some(input) => div()
                .w(px(widths[0]))
                .flex_none()
                .h(px(22.0))
                .child(ui::rename_field(theme, input))
                .into_any_element(),
            None => div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .w(px(widths[0]))
                .flex_none()
                .overflow_hidden()
                .text_color(rgb(theme.text))
                .child(ui::leading_icon_badge(
                    "icons/queries.svg",
                    theme.icon_queries,
                    ui::DETAIL_BADGE_SIZE,
                ))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-0")),
                    file.name.clone(),
                ))
                .child(ui::tag_chip(QUERY_TAG.to_string(), theme))
                .into_any_element(),
        };
        let click_key = key.clone();
        let menu_key = key.clone();
        let row = ui::detail_row(
            SharedString::from(format!("query-file-{index}")),
            selected,
            theme,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_query(&click_key, event.click_count(), event.modifiers(), cx);
            if this.click_opens(event.click_count(), event.modifiers()) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                if !this.query_selection.contains(&menu_key) {
                    this.select_query_one(index);
                }
                this.context_menu = Some(ContextMenu {
                    target: ContextTarget::QueryFile { index },
                    position: event.position,
                });
                cx.notify();
            }),
        )
        .child(name)
        .child(
            div()
                .w(px(widths[1]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-1")),
                    query_modified_text(file),
                )),
        )
        .child(
            div()
                .w(px(widths[2]))
                .flex_none()
                .text_color(rgb(theme.text_muted))
                .child(ui::detail_cell_text(
                    SharedString::from(format!("query-cell-{index}-2")),
                    human_size(file.size),
                )),
        );
        list_ops::row_with_rect(cx.weak_entity(), row, MarqueeTarget::Queries, key)
    }

    /// The Queries 平铺网格: the in-scope saved queries in a column-major grid (items fill a column
    /// top-to-bottom, then wrap to the next column), scrolling horizontally like the object list.
    fn render_query_tiles(
        &self,
        visible: &[usize],
        theme: Theme,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let width = ui::grid_item_width(
            visible
                .iter()
                .filter_map(|index| self.query_files.get(*index))
                .map(|file| ui::approx_text_width(&file.name))
                .fold(0.0, f32::max),
        );
        let rows = self.query_grid.rows_per_column();
        let mut columns = ui::grid_columns();
        let mut column = ui::grid_column();
        let mut count = 0usize;
        for &index in visible {
            if count == rows {
                columns = columns.child(column);
                column = ui::grid_column();
                count = 0;
            }
            column = column.child(self.query_grid_item(index, theme, width, cx));
            count += 1;
        }
        if count > 0 {
            columns = columns.child(column);
        }

        let mut grid = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_hidden();
        grid = grid.child(self.query_grid.scroller("query-grid-scroll", columns));
        if self.query_grid.overflows() {
            grid = grid.child(
                self.query_grid
                    .scrollbar("query-grid-hscrollbar", theme)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                            this.query_grid_begin(event.position.x, cx);
                        }),
                    ),
            );
        }
        grid.into_any_element()
    }

    /// One item of a 平铺网格 column: the query's icon badge and name.
    fn query_grid_item(
        &self,
        index: usize,
        theme: Theme,
        width: f32,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let file = &self.query_files[index];
        let key = query_file_key(file);
        let selected = self.query_selection.contains(&key);
        let title: AnyElement = match self.query_rename_input(index) {
            Some(input) => div()
                .flex_1()
                .min_w(px(0.0))
                .h(px(20.0))
                .child(ui::rename_field(theme, input))
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_color(rgb(theme.text))
                .child(file.name.clone())
                .into_any_element(),
        };
        let click_key = key.clone();
        let menu_key = key.clone();
        let item = ui::grid_item_sized(
            SharedString::from(format!("query-tile-{index}")),
            selected,
            theme,
            width,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            this.hit_query(&click_key, event.click_count(), event.modifiers(), cx);
            if this.click_opens(event.click_count(), event.modifiers()) {
                this.open_saved_query(index, cx);
            }
            cx.notify();
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
                cx.stop_propagation();
                if !this.query_selection.contains(&menu_key) {
                    this.select_query_one(index);
                }
                this.context_menu = Some(ContextMenu {
                    target: ContextTarget::QueryFile { index },
                    position: event.position,
                });
                cx.notify();
            }),
        )
        .child(ui::leading_icon_badge(
            "icons/queries.svg",
            theme.icon_queries,
            16.0,
        ))
        .child(title);
        list_ops::row_with_rect(cx.weak_entity(), item, MarqueeTarget::Queries, key)
    }

    /// Start dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_begin(&mut self, mouse_x: Pixels, cx: &mut Context<'_, Self>) {
        if self.query_grid.begin(mouse_x) {
            cx.notify();
        }
    }

    /// Continue dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        if self.query_grid.drag(event) {
            cx.notify();
        }
    }

    /// Finish dragging the 平铺 grid's horizontal scrollbar.
    fn query_grid_end(&mut self, cx: &mut Context<'_, Self>) {
        if self.query_grid.end() {
            cx.notify();
        }
    }

    /// The in-place rename editor for a query file, when it belongs to this row.
    fn query_rename_input(&self, index: usize) -> Option<Entity<TextInput>> {
        self.query_rename
            .as_ref()
            .filter(|edit| edit.index == index)
            .map(|edit| edit.input.clone())
    }

    /// Begin dragging the splitter between the query editor and its result panel, capturing the
    /// pointer y and the panel height it started at.
    pub(super) fn begin_query_split_drag(&mut self, pointer_y: Pixels, cx: &mut Context<'_, Self>) {
        self.query_split_drag = Some((f32::from(pointer_y), self.query_result_height));
        cx.notify();
    }

    /// Continue dragging the result splitter. Routed from the window root, so the drag survives
    /// the pointer leaving the 5px divider.
    pub(super) fn drag_query_split(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let Some((start_y, start_height)) = self.query_split_drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.query_split_drag = None;
            cx.notify();
            return;
        }
        // The divider sits on the panel's top edge, so dragging up grows the panel (a negative
        // pointer delta must increase the height, not shrink it).
        let delta = f32::from(event.position.y) - start_y;
        self.query_result_height =
            (start_height - delta).clamp(QUERY_RESULT_MIN_HEIGHT, QUERY_RESULT_MAX_HEIGHT);
        cx.notify();
    }

    /// Finish dragging the result splitter.
    pub(super) fn end_query_split_drag(&mut self, cx: &mut Context<'_, Self>) {
        if self.query_split_drag.take().is_some() {
            cx.notify();
        }
    }
}

/// The tag chip shown beside a saved query's name.
const QUERY_TAG: &str = "SQL";

/// A saved query's modified timestamp, or `--` when the filesystem did not report one.
fn query_modified_text(file: &QueryFileInfo) -> String {
    file.modified
        .map(format_file_time)
        .unwrap_or_else(|| "--".to_string())
}
