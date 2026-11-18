use gpui::WindowAppearance;
use rustgrid_config::ThemeSetting;

/// A per-connection environment colour, picked from the connection's right-click menu (参考
/// Navicat). It tints the connection's subtree in the tree and its tabs so a production
/// environment is hard to miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionColor {
    Red,
    Yellow,
    Green,
}

impl ConnectionColor {
    /// Every colour offered by the menu, in display order.
    pub const ALL: [ConnectionColor; 3] = [Self::Red, Self::Yellow, Self::Green];

    /// The stable id persisted in `settings.json`.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Yellow => "yellow",
            Self::Green => "green",
        }
    }

    /// Parse a persisted colour id; an unknown value means "no colour".
    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "red" => Some(Self::Red),
            "yellow" => Some(Self::Yellow),
            "green" => Some(Self::Green),
            _ => None,
        }
    }

    /// The solid accent for this colour, resolved against the active theme so it reads correctly
    /// in both light and dark mode. The hues are a true red / yellow / green (not the amber
    /// "warning" tone), so the environment colour is unambiguous.
    pub fn accent(self, theme: Theme) -> u32 {
        let dark = theme.is_dark();
        match self {
            Self::Red if dark => 0xf87171,
            Self::Red => 0xdc2626,
            Self::Yellow if dark => 0xfacc15,
            Self::Yellow => 0xeab308,
            Self::Green if dark => 0x4ade80,
            Self::Green => 0x16a34a,
        }
    }

    /// A translucent background of this colour, as an `rgba` value (`0xRRGGBBAA`). Laid over the
    /// theme's own background it gives the pastel Navicat paints behind a connection.
    pub fn tint(self, theme: Theme, alpha: u32) -> u32 {
        (self.accent(theme) << 8) | (alpha & 0xff)
    }

    /// The i18n key of this colour's label.
    pub const fn label_key(self) -> &'static str {
        match self {
            Self::Red => "color.red",
            Self::Yellow => "color.yellow",
            Self::Green => "color.green",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub window_bg: u32,
    pub titlebar_bg: u32,
    pub sidebar_bg: u32,
    pub editor_bg: u32,
    pub toolbar_bg: u32,
    pub header_bg: u32,
    pub row_alt_bg: u32,
    pub cell_edit_bg: u32,
    pub grid_selection_bg: u32,
    pub grid_selection_text: u32,
    pub dialog_bg: u32,
    pub dialog_face: u32,
    pub input_bg: u32,
    pub border: u32,
    pub grid_line: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_null: u32,
    pub button_bg: u32,
    pub button_hover_bg: u32,
    pub button_border: u32,
    pub button_default_border: u32,
    pub scroll_track: u32,
    pub scroll_thumb: u32,
    pub primary: u32,
    /// Brand accent used to brighten active state (selected main tab icon + label).
    pub brand: u32,
    /// Restrained brand tint used as the active main-tab background.
    pub brand_muted: u32,
    pub overlay: u32,
    pub tree_hover_bg: u32,
    pub tree_selected_bg: u32,
    pub tree_selected_text: u32,
    /// The connection tree's selected-row fill and text. More saturated than
    /// [`Theme::tree_selected_bg`] (which doubles as a subtle hover fill elsewhere), so a
    /// selected connection or database stays obvious over an environment-colour tint and while
    /// the pointer rests on the row.
    pub tree_active_bg: u32,
    pub tree_active_text: u32,
    pub chevron: u32,
    pub icon_connection: u32,
    pub icon_database: u32,
    pub icon_database_active: u32,
    pub icon_tables: u32,
    pub icon_views: u32,
    pub icon_functions: u32,
    /// Tint of the stored-procedure icon (the function icon reuses [`Theme::icon_functions`]).
    pub icon_procedure: u32,
    pub icon_queries: u32,
    pub icon_backups: u32,
    pub icon_users: u32,
    /// Tint of a connection-tree group's folder icon.
    pub icon_group: u32,
    /// A single table object (object list, table tabs, info pane). Kept in step with the
    /// connection tree's [`Theme::icon_tables`] so the same table keeps one colour everywhere.
    pub icon_table: u32,
    /// A single view object; kept in step with the connection tree's [`Theme::icon_views`].
    pub icon_view: u32,
    pub warning: u32,
    pub danger: u32,
    pub neutral: u32,
    /// SQL colors for the 选项 editor preview and other SQL snippets. They mirror gpui-kit's
    /// default highlight palette, which is also what the SQL editor and the DDL view paint with,
    /// so every SQL surface shares one set of colors.
    pub sql_keyword: u32,
    pub sql_string: u32,
    pub sql_number: u32,
    pub sql_comment: u32,
}

impl Theme {
    /// The light palette, aligned with shadcn/ui's default ("zinc") design tokens.
    ///
    /// The chrome is white and the structure comes from borders (`border`) and the grid's cell
    /// lines (`grid_line`), the way Navicat reads: white panels, visible separators, near-black
    /// text. Grey *fills* are reserved for the few surfaces that mean "secondary" — grid headers,
    /// the alternate row and hover states — so the window never washes out into one grey mass.
    pub const fn light() -> Self {
        Self {
            window_bg: 0xffffff,
            titlebar_bg: 0xffffff,
            sidebar_bg: 0xffffff,
            editor_bg: 0xffffff,
            toolbar_bg: 0xffffff,
            header_bg: 0xf4f4f5,
            row_alt_bg: 0xfafafa,
            cell_edit_bg: 0xfef9c3,
            grid_selection_bg: 0x3b82f6,
            grid_selection_text: 0xffffff,
            dialog_bg: 0xffffff,
            dialog_face: 0xffffff,
            input_bg: 0xffffff,
            border: 0xd4d4d8,
            grid_line: 0xe4e4e7,
            text: 0x09090b,
            text_muted: 0x52525b,
            text_null: 0x71717a,
            button_bg: 0xf4f4f5,
            button_hover_bg: 0xe4e4e7,
            button_border: 0xd4d4d8,
            button_default_border: 0xa1a1aa,
            scroll_track: 0xf4f4f5,
            scroll_thumb: 0xa1a1aa,
            primary: 0x18181b,
            brand: 0x2563eb,
            brand_muted: 0xdbeafe,
            overlay: 0x0000004d,
            tree_hover_bg: 0xf4f4f5,
            tree_selected_bg: 0xdbeafe,
            tree_selected_text: 0x09090b,
            tree_active_bg: 0x93c5fd,
            tree_active_text: 0x09090b,
            chevron: 0x71717a,
            icon_connection: 0x16a34a,
            icon_database: 0x71717a,
            icon_database_active: 0x16a34a,
            icon_tables: 0x16a34a,
            icon_views: 0x3b82f6,
            icon_functions: 0xd97706,
            icon_procedure: 0x7c3aed,
            icon_queries: 0x0d9488,
            icon_backups: 0xca8a04,
            icon_users: 0xd97706,
            icon_group: 0x71717a,
            icon_table: 0x16a34a,
            icon_view: 0x3b82f6,
            warning: 0xd97706,
            danger: 0xef4444,
            neutral: 0xa1a1aa,
            sql_keyword: 0x0433ff,
            sql_string: 0x036a07,
            sql_number: 0x0433ff,
            sql_comment: 0x007fff,
        }
    }

    /// The dark palette, aligned with shadcn/ui's default ("zinc") design tokens.
    pub const fn dark() -> Self {
        Self {
            window_bg: 0x09090b,
            titlebar_bg: 0x09090b,
            sidebar_bg: 0x09090b,
            editor_bg: 0x09090b,
            toolbar_bg: 0x09090b,
            header_bg: 0x18181b,
            row_alt_bg: 0x0c0c0e,
            cell_edit_bg: 0x4a3f1c,
            grid_selection_bg: 0x2563eb,
            grid_selection_text: 0xfafafa,
            dialog_bg: 0x09090b,
            dialog_face: 0x18181b,
            input_bg: 0x09090b,
            border: 0x27272a,
            grid_line: 0x27272a,
            text: 0xfafafa,
            text_muted: 0xa1a1aa,
            text_null: 0x52525b,
            button_bg: 0x27272a,
            button_hover_bg: 0x3f3f46,
            button_border: 0x27272a,
            button_default_border: 0xd4d4d8,
            scroll_track: 0x18181b,
            scroll_thumb: 0x52525b,
            primary: 0xfafafa,
            brand: 0x60a5fa,
            brand_muted: 0x172554,
            overlay: 0x000000a6,
            tree_hover_bg: 0x18181b,
            tree_selected_bg: 0x1e3a8a,
            tree_selected_text: 0xfafafa,
            tree_active_bg: 0x1d4ed8,
            tree_active_text: 0xfafafa,
            chevron: 0xa1a1aa,
            icon_connection: 0x4ade80,
            icon_database: 0xa1a1aa,
            icon_database_active: 0x4ade80,
            icon_tables: 0x4ade80,
            icon_views: 0x60a5fa,
            icon_functions: 0xfbbf24,
            icon_procedure: 0xa78bfa,
            icon_queries: 0x2dd4bf,
            icon_backups: 0xfacc15,
            icon_users: 0xfbbf24,
            icon_group: 0xa1a1aa,
            icon_table: 0x4ade80,
            icon_view: 0x60a5fa,
            warning: 0xfbbf24,
            danger: 0xef4444,
            neutral: 0x52525b,
            sql_keyword: 0x87b1f6,
            sql_string: 0xa3e09f,
            sql_number: 0xcc9e00,
            sql_comment: 0x9d9d9d,
        }
    }

    pub fn resolve(setting: ThemeSetting, appearance: WindowAppearance) -> Self {
        match setting {
            ThemeSetting::Light => Self::light(),
            ThemeSetting::Dark => Self::dark(),
            ThemeSetting::System => match appearance {
                WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            },
        }
    }

    /// Whether this palette is the dark one, used to keep gpui-kit's theme in lockstep.
    pub fn is_dark(&self) -> bool {
        self.window_bg == Self::dark().window_bg
    }
}
