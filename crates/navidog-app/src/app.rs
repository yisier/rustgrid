use std::sync::Arc;

use gpui::{
    Context, FocusHandle, KeyDownEvent, Render, ScrollHandle, SharedString, Window, div,
    prelude::*, px, rgb, rgba,
};
use navidog_config::ConfigStore;
use navidog_core::{Connection, ConnectionConfig, DriverRegistry, Error, PageRequest};

use crate::form::{ConnectionForm, FORM_FIELDS, FormAction, FormField};
use crate::runtime::Runtime;
use crate::session::{ConnectionNode, ConnectionStatus, DatabaseNode, GridState, Loadable};

struct FormFocus {
    name: FocusHandle,
    host: FocusHandle,
    port: FocusHandle,
    username: FocusHandle,
    password: FocusHandle,
    database: FocusHandle,
}

impl FormFocus {
    fn get(&self, field: FormField) -> &FocusHandle {
        match field {
            FormField::Name => &self.name,
            FormField::Host => &self.host,
            FormField::Port => &self.port,
            FormField::Username => &self.username,
            FormField::Password => &self.password,
            FormField::Database => &self.database,
        }
    }

    fn next(&self, field: FormField) -> &FocusHandle {
        let index = FORM_FIELDS
            .iter()
            .position(|item| *item == field)
            .unwrap_or(0);
        self.get(FORM_FIELDS[(index + 1) % FORM_FIELDS.len()])
    }

    fn previous(&self, field: FormField) -> &FocusHandle {
        let index = FORM_FIELDS
            .iter()
            .position(|item| *item == field)
            .unwrap_or(0);
        self.get(FORM_FIELDS[(index + FORM_FIELDS.len() - 1) % FORM_FIELDS.len()])
    }
}

pub struct AppView {
    registry: Arc<DriverRegistry>,
    config: Arc<ConfigStore>,
    runtime: Arc<Runtime>,
    connections: Vec<ConnectionNode>,
    grid: Option<GridState>,
    form: Option<ConnectionForm>,
    form_focus: FormFocus,
    page_size: u64,
    sidebar_scroll: ScrollHandle,
    grid_scroll: ScrollHandle,
}

impl AppView {
    pub fn new(
        registry: Arc<DriverRegistry>,
        config: Arc<ConfigStore>,
        runtime: Arc<Runtime>,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let connections = config
            .load_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| ConnectionNode {
                profile,
                password: None,
                status: ConnectionStatus::Disconnected,
                databases: Loadable::Idle,
                expanded: false,
            })
            .collect();

        Self {
            registry,
            config,
            runtime,
            connections,
            grid: None,
            form: None,
            form_focus: FormFocus {
                name: cx.focus_handle(),
                host: cx.focus_handle(),
                port: cx.focus_handle(),
                username: cx.focus_handle(),
                password: cx.focus_handle(),
                database: cx.focus_handle(),
            },
            page_size: 100,
            sidebar_scroll: ScrollHandle::new(),
            grid_scroll: ScrollHandle::new(),
        }
    }

    fn connection_arc(&self, index: usize) -> Option<Arc<dyn Connection>> {
        match &self.connections.get(index)?.status {
            ConnectionStatus::Connected(connection) => Some(connection.clone()),
            _ => None,
        }
    }

    fn toggle_connection(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let state = match self.connections.get(index).map(|node| &node.status) {
            Some(ConnectionStatus::Connected(_)) => 1,
            Some(ConnectionStatus::Connecting) => 2,
            _ => 0,
        };

        match state {
            1 => self.disconnect(index, cx),
            2 => {}
            _ => self.connect(index, cx),
        }
    }

    fn toggle_expand(&mut self, index: usize) {
        if let Some(node) = self.connections.get_mut(index)
            && matches!(&node.status, ConnectionStatus::Connected(_))
        {
            node.expanded = !node.expanded;
        }
    }

    fn connect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(profile) = self.connections.get(index).map(|node| node.profile.clone()) else {
            return;
        };
        let password = self
            .connections
            .get(index)
            .and_then(|node| node.password.clone());

        let Some(driver) = self.registry.get(&profile.driver) else {
            if let Some(node) = self.connections.get_mut(index) {
                node.status = ConnectionStatus::Failed(t!("error.driver_missing").to_string());
            }
            cx.notify();
            return;
        };

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Connecting;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let config = ConnectionConfig {
                driver: profile.driver.clone(),
                host: profile.host.clone(),
                port: profile.port,
                username: profile.username.clone(),
                password,
                database: profile.database.clone(),
                options: profile.options.clone(),
            };

            let result = match runtime
                .spawn(async move { driver.connect(&config).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(connection) => {
                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Connected(Arc::from(connection));
                            node.expanded = true;
                        }
                        view.load_databases(index, cx);
                    }
                    Err(error) => {
                        if let Some(node) = view.connections.get_mut(index) {
                            node.status = ConnectionStatus::Failed(error.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn disconnect(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let connection = self.connection_arc(index);

        if let Some(node) = self.connections.get_mut(index) {
            node.status = ConnectionStatus::Disconnected;
            node.databases = Loadable::Idle;
            node.expanded = false;
        }

        if let Some(connection) = connection {
            let runtime = self.runtime.clone();
            cx.spawn(async move |_this, _cx| {
                let _ = runtime.spawn(async move { connection.close().await }).await;
            })
            .detach();
        }

        cx.notify();
    }

    fn load_databases(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(connection) = self.connection_arc(index) else {
            return;
        };

        if let Some(node) = self.connections.get_mut(index) {
            node.databases = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_databases().await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(index) {
                    node.databases = match result {
                        Ok(databases) => Loadable::Loaded(
                            databases
                                .into_iter()
                                .map(|database| DatabaseNode {
                                    name: database.name,
                                    tables: Loadable::Idle,
                                    expanded: false,
                                })
                                .collect(),
                        ),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn toggle_database(
        &mut self,
        connection_index: usize,
        database_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        let Some(database_name) =
            self.connections
                .get(connection_index)
                .and_then(|node| match &node.databases {
                    Loadable::Loaded(databases) => {
                        databases.get(database_index).map(|db| db.name.clone())
                    }
                    _ => None,
                })
        else {
            return;
        };

        let mut should_load = false;
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            if database.expanded {
                database.expanded = false;
            } else {
                database.expanded = true;
                if matches!(database.tables, Loadable::Idle | Loadable::Failed(_)) {
                    should_load = true;
                }
            }
        }

        if should_load {
            self.load_tables(
                connection_index,
                database_index,
                connection,
                database_name,
                cx,
            );
        }

        cx.notify();
    }

    fn load_tables(
        &mut self,
        connection_index: usize,
        database_index: usize,
        connection: Arc<dyn Connection>,
        database_name: String,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(node) = self.connections.get_mut(connection_index)
            && let Loadable::Loaded(databases) = &mut node.databases
            && let Some(database) = databases.get_mut(database_index)
        {
            database.tables = Loadable::Loading;
        }

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move { connection.list_tables(&database_name).await })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(node) = view.connections.get_mut(connection_index)
                    && let Loadable::Loaded(databases) = &mut node.databases
                    && let Some(database) = databases.get_mut(database_index)
                {
                    database.tables = match result {
                        Ok(tables) => Loadable::Loaded(tables),
                        Err(error) => Loadable::Failed(error.to_string()),
                    };
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn select_table(
        &mut self,
        connection_index: usize,
        database: String,
        table: String,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(connection) = self.connection_arc(connection_index) else {
            return;
        };

        self.grid = Some(GridState {
            connection,
            database,
            table,
            page_index: 0,
            page_size: self.page_size,
            loading: false,
            error: None,
            columns: Vec::new(),
            rows: Vec::new(),
            total_rows: None,
        });

        self.load_page(cx);
    }

    fn load_page(&mut self, cx: &mut Context<'_, Self>) {
        let Some(grid) = self.grid.as_mut() else {
            return;
        };

        grid.loading = true;
        grid.error = None;

        let connection = grid.connection.clone();
        let database = grid.database.clone();
        let table = grid.table.clone();
        let page_index = grid.page_index;
        let page_size = grid.page_size;

        cx.notify();

        let runtime = self.runtime.clone();
        cx.spawn(async move |this, cx| {
            let result = match runtime
                .spawn(async move {
                    connection
                        .fetch_page(&database, &table, PageRequest::new(page_index, page_size))
                        .await
                })
                .await
            {
                Ok(inner) => inner,
                Err(error) => Err(Error::other(error)),
            };

            let _ = this.update(cx, |view, cx| {
                if let Some(grid) = view.grid.as_mut() {
                    grid.loading = false;
                    match result {
                        Ok(page) => {
                            grid.columns = page.columns;
                            grid.rows = page.rows;
                            grid.total_rows = page.total_rows;
                            grid.error = None;
                        }
                        Err(error) => {
                            grid.error = Some(error.to_string());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self.grid.as_mut()
            && grid.has_next()
        {
            grid.page_index += 1;
        }
        self.load_page(cx);
    }

    fn prev_page(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(grid) = self.grid.as_mut() {
            grid.page_index = grid.page_index.saturating_sub(1);
        }
        self.load_page(cx);
    }

    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.load_page(cx);
    }

    fn save_form(&mut self, cx: &mut Context<'_, Self>) {
        let Some(form) = self.form.take() else {
            return;
        };

        let profile = form.to_profile();
        let password = if form.password.is_empty() {
            None
        } else {
            Some(form.password.clone())
        };

        self.connections.push(ConnectionNode {
            profile,
            password,
            status: ConnectionStatus::Disconnected,
            databases: Loadable::Idle,
            expanded: false,
        });

        let profiles: Vec<_> = self
            .connections
            .iter()
            .map(|node| node.profile.clone())
            .collect();
        let _ = self.config.save_profiles(&profiles);

        let index = self.connections.len() - 1;
        self.connect(index, cx);
        cx.notify();
    }

    fn form_key(
        &mut self,
        field: FormField,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let action = match self.form.as_mut() {
            Some(form) => form.apply_key(field, event),
            None => return,
        };

        match action {
            FormAction::Changed | FormAction::Ignore => {}
            FormAction::Submit => self.save_form(cx),
            FormAction::FocusNext => window.focus(self.form_focus.next(field)),
            FormAction::FocusPrev => window.focus(self.form_focus.previous(field)),
        }

        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let mut list = div().flex().flex_col().gap_1();

        if self.connections.is_empty() {
            list = list.child(div().child(t!("sidebar.no_connections").to_string()));
        }

        for (index, node) in self.connections.iter().enumerate() {
            list = list.child(self.render_connection(index, node, cx));
        }

        div()
            .flex()
            .flex_col()
            .w(px(260.0))
            .h_full()
            .flex_none()
            .bg(rgb(0x252526))
            .border_r_1()
            .border_color(rgb(0x3c3c3c))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .p_2()
                    .child(div().text_xl().child(t!("sidebar.connections").to_string()))
                    .child(
                        div()
                            .id("new-connection")
                            .cursor_pointer()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x3a3d41))
                            .on_click(cx.listener(|this, _event, window, cx| {
                                this.form = Some(ConnectionForm::default());
                                window.focus(&this.form_focus.name);
                                cx.notify();
                            }))
                            .child(t!("sidebar.new_connection").to_string()),
                    ),
            )
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.sidebar_scroll)
                    .p_2()
                    .child(list),
            )
            .child(div().p_2().child(format!(
                "{}: {}",
                t!("sidebar.drivers"),
                self.registry.len()
            )))
    }

    fn render_connection(
        &self,
        index: usize,
        node: &ConnectionNode,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let dot = match &node.status {
            ConnectionStatus::Connected(_) => 0x4ec9b0,
            ConnectionStatus::Connecting => 0xdcdcaa,
            ConnectionStatus::Failed(_) => 0xf14c4c,
            ConnectionStatus::Disconnected => 0x808080,
        };

        let button_label = match &node.status {
            ConnectionStatus::Connected(_) => t!("connection.disconnect").to_string(),
            ConnectionStatus::Connecting => t!("connection.connecting").to_string(),
            _ => t!("connection.connect").to_string(),
        };

        let row = div()
            .id(SharedString::from(format!("conn-{index}")))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_1()
            .px_1()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, _cx| this.toggle_expand(index)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .overflow_hidden()
                    .child(
                        div()
                            .w(px(8.0))
                            .h(px(8.0))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(dot)),
                    )
                    .child(div().overflow_hidden().child(node.profile.name.clone())),
            )
            .child(
                div()
                    .id(SharedString::from(format!("conn-btn-{index}")))
                    .flex_none()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgb(0x3a3d41))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.toggle_connection(index, cx);
                    }))
                    .child(button_label),
            );

        let mut sub = div().flex().flex_col().pl_4();

        if let ConnectionStatus::Failed(error) = &node.status {
            sub = sub.child(div().text_color(rgb(0xf14c4c)).child(error.clone()));
        }

        if node.expanded {
            match &node.databases {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(div().child(t!("common.loading").to_string()));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(div().text_color(rgb(0xf14c4c)).child(error.clone()));
                }
                Loadable::Loaded(databases) => {
                    for (database_index, database) in databases.iter().enumerate() {
                        sub = sub.child(self.render_database(index, database_index, database, cx));
                    }
                }
            }
        }

        div().flex().flex_col().gap_1().child(row).child(sub)
    }

    fn render_database(
        &self,
        connection_index: usize,
        database_index: usize,
        database: &DatabaseNode,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let marker = if database.expanded { "v" } else { ">" };

        let row = div()
            .id(SharedString::from(format!(
                "db-{connection_index}-{database_index}"
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_1()
            .py_1()
            .rounded_md()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.toggle_database(connection_index, database_index, cx);
            }))
            .child(div().child(marker))
            .child(div().child(database.name.clone()));

        let mut sub = div().flex().flex_col().pl_4();

        if database.expanded {
            match &database.tables {
                Loadable::Idle => {}
                Loadable::Loading => {
                    sub = sub.child(div().child(t!("common.loading").to_string()));
                }
                Loadable::Failed(error) => {
                    sub = sub.child(div().text_color(rgb(0xf14c4c)).child(error.clone()));
                }
                Loadable::Loaded(tables) => {
                    for (table_index, table) in tables.iter().enumerate() {
                        let is_view = matches!(table.kind, navidog_core::ObjectKind::View);
                        let label = if is_view {
                            format!("{} {}", t!("common.view"), table.name)
                        } else {
                            table.name.clone()
                        };

                        let database_name = database.name.clone();
                        let table_name = table.name.clone();

                        sub = sub.child(
                            div()
                                .id(SharedString::from(format!(
                                    "tbl-{connection_index}-{database_index}-{table_index}"
                                )))
                                .px_1()
                                .py_1()
                                .rounded_md()
                                .cursor_pointer()
                                .on_click(cx.listener(move |this, _event, _window, cx| {
                                    this.select_table(
                                        connection_index,
                                        database_name.clone(),
                                        table_name.clone(),
                                        cx,
                                    );
                                }))
                                .child(label),
                        );
                    }
                }
            }
        }

        div().flex().flex_col().child(row).child(sub)
    }

    fn render_content(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let mut content = div().flex().flex_col().flex_1().h_full().overflow_hidden();

        match &self.grid {
            Some(grid) => content = content.child(self.render_grid(grid, cx)),
            None => {
                content = content.child(
                    div()
                        .p_4()
                        .text_xl()
                        .child(t!("content.no_table_selected").to_string()),
                );
            }
        }

        content
    }

    fn render_grid(&self, grid: &GridState, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let mut header = div().flex().flex_row().bg(rgb(0x2d2d30));
        for column in &grid.columns {
            header = header.child(
                div()
                    .w(px(160.0))
                    .flex_none()
                    .px_2()
                    .py_1()
                    .border_r_1()
                    .border_color(rgb(0x3c3c3c))
                    .child(column.name.clone()),
            );
        }

        let mut rows = div().flex().flex_col();
        for (row_index, row) in grid.rows.iter().enumerate() {
            let background = if row_index % 2 == 1 {
                rgb(0x232324)
            } else {
                rgb(0x1e1e1e)
            };
            let mut row_element = div().flex().flex_row().bg(background);
            for cell in row {
                row_element = row_element.child(
                    div()
                        .w(px(160.0))
                        .flex_none()
                        .px_2()
                        .py_1()
                        .overflow_hidden()
                        .child(cell.as_display()),
                );
            }
            rows = rows.child(row_element);
        }

        let table = div()
            .id("grid-scroll")
            .flex_1()
            .w_full()
            .overflow_scroll()
            .track_scroll(&self.grid_scroll)
            .child(div().flex().flex_col().child(header).child(rows));

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(self.render_toolbar(grid, cx))
            .child(table)
    }

    fn render_toolbar(&self, grid: &GridState, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let title = format!("{}.{}", grid.database, grid.table);
        let page_label = format!("{} {}", t!("grid.page"), grid.page_index + 1);
        let total = match grid.total_rows {
            Some(total) => format!("{}: {}", t!("content.rows"), total),
            None => String::new(),
        };

        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap_3()
            .p_2()
            .bg(rgb(0x252526))
            .border_b_1()
            .border_color(rgb(0x3c3c3c))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(div().text_xl().child(title))
                    .child(div().child(total)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("grid-prev")
                            .cursor_pointer()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x3a3d41))
                            .on_click(cx.listener(|this, _event, _window, cx| this.prev_page(cx)))
                            .child(t!("grid.prev").to_string()),
                    )
                    .child(div().child(page_label))
                    .child(
                        div()
                            .id("grid-next")
                            .cursor_pointer()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x3a3d41))
                            .on_click(cx.listener(|this, _event, _window, cx| this.next_page(cx)))
                            .child(t!("grid.next").to_string()),
                    )
                    .child(
                        div()
                            .id("grid-refresh")
                            .cursor_pointer()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .bg(rgb(0x3a3d41))
                            .on_click(cx.listener(|this, _event, _window, cx| this.refresh(cx)))
                            .child(t!("grid.refresh").to_string()),
                    ),
            )
    }

    fn render_dialog(
        &self,
        form: &ConnectionForm,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x00000099))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .w(px(420.0))
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0x2d2d30))
                    .child(div().text_xl().child(t!("form.title").to_string()))
                    .child(self.render_field(
                        FormField::Name,
                        t!("form.name").to_string(),
                        form.value(FormField::Name),
                        false,
                        window,
                        cx,
                    ))
                    .child(self.render_field(
                        FormField::Host,
                        t!("form.host").to_string(),
                        form.value(FormField::Host),
                        false,
                        window,
                        cx,
                    ))
                    .child(self.render_field(
                        FormField::Port,
                        t!("form.port").to_string(),
                        form.value(FormField::Port),
                        false,
                        window,
                        cx,
                    ))
                    .child(self.render_field(
                        FormField::Username,
                        t!("form.username").to_string(),
                        form.value(FormField::Username),
                        false,
                        window,
                        cx,
                    ))
                    .child(self.render_field(
                        FormField::Password,
                        t!("form.password").to_string(),
                        form.value(FormField::Password),
                        true,
                        window,
                        cx,
                    ))
                    .child(self.render_field(
                        FormField::Database,
                        t!("form.database").to_string(),
                        form.value(FormField::Database),
                        false,
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .id("form-cancel")
                                    .cursor_pointer()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x3a3d41))
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.form = None;
                                        cx.notify();
                                    }))
                                    .child(t!("form.cancel").to_string()),
                            )
                            .child(
                                div()
                                    .id("form-save")
                                    .cursor_pointer()
                                    .px_3()
                                    .py_1()
                                    .rounded_md()
                                    .bg(rgb(0x0e639c))
                                    .on_click(
                                        cx.listener(|this, _event, _window, cx| this.save_form(cx)),
                                    )
                                    .child(t!("form.save").to_string()),
                            ),
                    ),
            )
    }

    fn render_field(
        &self,
        field: FormField,
        label: String,
        value: &str,
        masked: bool,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let handle = self.form_focus.get(field);
        let focused = handle.is_focused(window);
        let shown = if masked {
            "*".repeat(value.chars().count())
        } else {
            value.to_string()
        };
        let shown = if focused { format!("{shown}|") } else { shown };
        let focus_handle = handle.clone();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().child(label))
            .child(
                div()
                    .id(SharedString::from(format!("field-{}", field as usize)))
                    .track_focus(handle)
                    .cursor_text()
                    .on_key_down(cx.listener(move |this, event, window, cx| {
                        this.form_key(field, event, window, cx)
                    }))
                    .on_click(cx.listener(move |_this, _event, window, _cx| {
                        window.focus(&focus_handle);
                    }))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .px_2()
                    .rounded_md()
                    .bg(rgb(0x1e1e1e))
                    .border_1()
                    .border_color(rgb(0x3c3c3c))
                    .child(shown),
            )
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let mut root = div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .text_color(rgb(0xd4d4d4))
            .child(self.render_sidebar(cx))
            .child(self.render_content(cx));

        if let Some(form) = self.form.as_ref() {
            root = root.child(self.render_dialog(form, window, cx));
        }

        root
    }
}
