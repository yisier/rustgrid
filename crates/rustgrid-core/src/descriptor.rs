//! Presentation and capability metadata for one driver.
//!
//! [`DriverDescriptor`] is the single source of truth for how the UI labels, icons, orders and
//! gates a driver. The app must not match on the engine id; it reads the descriptor instead.
//! [`Driver::descriptor`](crate::Driver::descriptor) builds one from the historical trait methods
//! by default, so adding it changes no behavior until an engine opts in.

use crate::capability::DriverCapabilities;
use crate::driver::DatabaseEditorSpec;
use crate::model::{DriverId, IndexDef};

/// How a driver's icon is drawn in the connection tree and menus.
///
/// Every style paints the engine's mark as a white glyph on a solid 16px badge, so it stays legible
/// on a tinted (environment-coloured) row as well as on the plain background; the variants only
/// differ in the badge's colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverIconStyle {
    /// The badge carries the connection status colour.
    Plain,
    /// The badge carries the connection status colour (the MySQL/MariaDB brand marks).
    SolidBadge,
    /// The badge carries the engine's brand colour while the connection is open, and the status
    /// colour while it is closed (SQLite, SQL Server, PostgreSQL, Oracle).
    Brand(u32),
}

/// A page (tab) of the New/Edit Connection window, and where an engine-specific field is shown.
///
/// The vocabulary is engine-agnostic: a driver declares which pages it shows and where its own
/// fields go, so the form never matches on an engine id (mirrors [`DatabaseEditorTab`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConnectionPage {
    /// The first tab: the connection's identity (alias, network address or file, credentials).
    General,
    /// TLS / SSL.
    Tls,
    /// SSH / SOCKS5 / HTTP tunnel.
    Tunnel,
    /// Timeouts, read-only, session init SQL, plus engine fields placed here.
    Advanced,
}

impl ConnectionPage {
    /// i18n key of the page's tab label.
    pub fn label_key(self) -> &'static str {
        match self {
            ConnectionPage::General => "form.tab.general",
            ConnectionPage::Tls => "form.tab.tls",
            ConnectionPage::Tunnel => "form.tab.tunnel",
            ConnectionPage::Advanced => "form.tab.advanced",
        }
    }

    /// A stable id fragment for the tab's element id.
    pub fn id(self) -> &'static str {
        match self {
            ConnectionPage::General => "general",
            ConnectionPage::Tls => "tls",
            ConnectionPage::Tunnel => "tunnel",
            ConnectionPage::Advanced => "advanced",
        }
    }
}

/// The shape of the first (常规) page: which standard fields it shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionHomePage {
    /// Alias + network address + port + credentials + default database (a network engine).
    Network,
    /// Alias + database file (a file-based engine, e.g. SQLite).
    File,
    /// Alias + the generic ODBC fields (driver name, DSN, connection string, engine hint).
    Odbc,
}

/// A standard form field whose label a driver may rename for its engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStandardField {
    Name,
    Host,
    Port,
    Username,
    Password,
    /// The "default database" field (Oracle renames it to a service name, SQL Server to the
    /// initial database, ...).
    Database,
    /// The file-based engine's database file.
    File,
    /// The 高级 page's connect timeout (SQLite renames it to its lock/busy timeout).
    ConnectTimeout,
    /// The 高级 page's query timeout.
    QueryTimeout,
    /// The 高级 page's keep-alive interval.
    Keepalive,
    /// The 高级 page's read-only toggle.
    ReadOnly,
    /// The 高级 page's session-init SQL.
    InitSql,
}

/// A per-engine rename/hint of one standard field. Fields a driver does not list keep the app's
/// default label.
#[derive(Debug, Clone, Copy)]
pub struct ConnectionFieldLabel {
    pub field: ConnectionStandardField,
    /// i18n key of the field's label.
    pub label_key: &'static str,
    /// i18n key of the field's label-row hint, if any.
    pub hint_key: Option<&'static str>,
    /// Show the standard field only while `options[0]` is one of the listed values (`None` →
    /// always shown). Oracle uses it to hide 服务名/SID for the connection types that ignore it
    /// (`tns` / `connect_string`); an unset option counts as the controlling `Select`'s first
    /// choice.
    pub visible_when: Option<(&'static str, &'static [&'static str])>,
    /// Whether the field is required while it is visible. `false` keeps the app's historical
    /// optional default (the engine's 数据库 field).
    pub required: bool,
}

/// Where an engine option field is drawn on the 常规 page.
///
/// The standard page is rendered in blocks (identity → address → target → credentials → transport).
/// An engine that wants its controlling select above the address (or its alias below the address)
/// declares the placement instead of the app matching on the engine id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionFieldPlacement {
    /// Above the standard network address block (Oracle's 连接类型).
    Leading,
    /// After the network address block and before the credentials (Oracle's 服务名/SID radio, TNS
    /// alias and tnsnames.ora directory).
    Target,
    /// The historical position: after the credentials and the standard database field.
    Trailing,
}

/// One choice of a [`ConnectionFieldKind::Select`] field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionFieldChoice {
    /// The value stored in `ConnectionProfile::options`.
    pub value: &'static str,
    /// i18n key of the choice's label.
    pub label_key: &'static str,
}

/// How one engine-specific connection field is edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionFieldKind {
    /// A plain single-line text field.
    Text,
    /// A masked single-line text field (a password or passphrase).
    Secret,
    /// A directory path with a trailing 浏览 (browse) button.
    Folder,
    /// A dropdown with a fixed set of choices; the first choice is the default.
    Select(Vec<ConnectionFieldChoice>),
    /// A horizontal radio group; the first choice is the default.
    Radio(Vec<ConnectionFieldChoice>),
    /// A multi-line editor (the advanced Oracle connect descriptor).
    Multiline,
}

/// One engine-specific field on the connection form.
///
/// The field reads and writes a single [`ConnectionProfile::options`] entry, so declaring a new
/// field needs no change to `connections.json` and no migration. Labels and hints are i18n keys
/// (a driver crate has no locale files of its own, so it hands the app the keys to resolve).
#[derive(Debug, Clone)]
pub struct ConnectionFieldSpec {
    /// The `ConnectionProfile::options` key this field reads and writes.
    pub key: &'static str,
    /// i18n key of the field's label.
    pub label_key: &'static str,
    /// i18n key of an optional hint shown on the label row.
    pub hint_key: Option<&'static str>,
    /// How the field is edited.
    pub kind: ConnectionFieldKind,
    /// Which page the field is shown on.
    pub page: ConnectionPage,
    /// Show the field only while `profile.options[0]` equals `1` (e.g. Oracle's TNS alias field is
    /// shown while the connection type is `tns`). A field that already holds a value is always
    /// shown, so a legacy profile can never hide an active option.
    pub visible_when: Option<(&'static str, &'static str)>,
    /// Where the field is drawn on 常规.
    pub placement: ConnectionFieldPlacement,
    /// Whether the field must be filled while it is visible. The form flags it inline like a
    /// required standard field.
    pub required: bool,
}

/// Which of the standard 高级 fields an engine actually honors. The form hides the ones an engine
/// leaves off, so a driver never shows a dead knob (e.g. Oracle has no keep-alive or connect
/// timeout, ODBC reads none of them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdvancedFields {
    /// 连接超时 / connect timeout.
    pub connect_timeout: bool,
    /// 查询超时 / per-statement query timeout.
    pub query_timeout: bool,
    /// 保持连接 / TCP keep-alive.
    pub keepalive: bool,
    /// 只读安全模式 / read-only session.
    pub read_only: bool,
    /// 连接初始化 SQL / session init SQL.
    pub init_sql: bool,
}

impl AdvancedFields {
    /// Every standard 高级 field (MySQL, MariaDB, PostgreSQL, SQL Server).
    pub const ALL: Self = Self {
        connect_timeout: true,
        query_timeout: true,
        keepalive: true,
        read_only: true,
        init_sql: true,
    };

    /// None of them (the generic ODBC driver reads no advanced settings).
    pub const NONE: Self = Self {
        connect_timeout: false,
        query_timeout: false,
        keepalive: false,
        read_only: false,
        init_sql: false,
    };
}

impl Default for AdvancedFields {
    fn default() -> Self {
        Self::ALL
    }
}

/// Which pages the connection form shows for an engine, how its 常规 page is shaped, how it renames
/// the standard fields, and where its own option fields go. The form stays engine-agnostic by
/// reading this instead of matching on the engine id.
#[derive(Debug, Clone)]
pub struct ConnectionFormSpec {
    /// The shape of the 常规 page.
    pub home: ConnectionHomePage,
    /// The tabs shown after 常规, in order.
    pub tabs: Vec<ConnectionPage>,
    /// Per-engine renames/hints for the standard fields.
    pub labels: Vec<ConnectionFieldLabel>,
    /// Standard fields the engine treats as optional: no `*` on the label and no required-field
    /// validation. (Redis user names are only needed for ACL accounts, so a blank one means the
    /// `default` user / password-only `AUTH`.)
    pub optional_fields: Vec<ConnectionStandardField>,
    /// Engine-specific option fields, each placed on a page.
    pub options: Vec<ConnectionFieldSpec>,
    /// Which standard 高级 fields the engine honors; the rest are hidden.
    pub advanced: AdvancedFields,
}

impl Default for ConnectionFormSpec {
    /// The standard network form: 常规 + TLS + 隧道 + 高级, with no engine fields. This is what a
    /// plain network engine (MySQL, PostgreSQL, ...) shows.
    fn default() -> Self {
        Self {
            home: ConnectionHomePage::Network,
            tabs: vec![
                ConnectionPage::Tls,
                ConnectionPage::Tunnel,
                ConnectionPage::Advanced,
            ],
            labels: Vec::new(),
            optional_fields: Vec::new(),
            options: Vec::new(),
            advanced: AdvancedFields::ALL,
        }
    }
}

/// One sub-tab of the table designer.
///
/// The vocabulary is engine-agnostic: a driver declares which tabs it shows (see
/// [`TableDesignerSpec`]), so the designer never matches on an engine id. A tab an engine does not
/// list is not rendered at all — a tab it lists but has no data for simply shows its empty grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesignerTab {
    /// The table's columns.
    Fields,
    /// Secondary indexes.
    Indexes,
    /// Foreign-key constraints.
    ForeignKeys,
    /// Unique constraints (SQL Server keeps these apart from plain indexes).
    UniqueKeys,
    /// Check constraints.
    Checks,
    /// PostgreSQL exclusion constraints.
    Excludes,
    /// PostgreSQL rewrite rules.
    Rules,
    /// Table triggers.
    Triggers,
    /// Table-level storage options.
    Options,
    /// MySQL/MariaDB partitioning.
    Partition,
    /// The table's comment.
    Comment,
    /// The statement Save would run.
    Sql,
}

impl DesignerTab {
    /// Every tab, in the designer's default order.
    pub const ALL: [DesignerTab; 12] = [
        DesignerTab::Fields,
        DesignerTab::Indexes,
        DesignerTab::ForeignKeys,
        DesignerTab::UniqueKeys,
        DesignerTab::Checks,
        DesignerTab::Excludes,
        DesignerTab::Rules,
        DesignerTab::Triggers,
        DesignerTab::Options,
        DesignerTab::Partition,
        DesignerTab::Comment,
        DesignerTab::Sql,
    ];

    /// i18n key of the tab's label.
    pub fn label_key(self) -> &'static str {
        match self {
            DesignerTab::Fields => "design.tab.fields",
            DesignerTab::Indexes => "design.tab.indexes",
            DesignerTab::ForeignKeys => "design.tab.foreign_keys",
            DesignerTab::UniqueKeys => "design.tab.unique_keys",
            DesignerTab::Checks => "design.tab.checks",
            DesignerTab::Excludes => "design.tab.excludes",
            DesignerTab::Rules => "design.tab.rules",
            DesignerTab::Triggers => "design.tab.triggers",
            DesignerTab::Options => "design.tab.options",
            DesignerTab::Partition => "design.tab.partition",
            DesignerTab::Comment => "design.tab.comment",
            DesignerTab::Sql => "design.tab.sql",
        }
    }

    /// A stable id fragment for the tab's element id.
    pub fn id(self) -> &'static str {
        match self {
            DesignerTab::Fields => "design-tab-fields",
            DesignerTab::Indexes => "design-tab-indexes",
            DesignerTab::ForeignKeys => "design-tab-foreign-keys",
            DesignerTab::UniqueKeys => "design-tab-unique-keys",
            DesignerTab::Checks => "design-tab-checks",
            DesignerTab::Excludes => "design-tab-excludes",
            DesignerTab::Rules => "design-tab-rules",
            DesignerTab::Triggers => "design-tab-triggers",
            DesignerTab::Options => "design-tab-options",
            DesignerTab::Partition => "design-tab-partition",
            DesignerTab::Comment => "design-tab-comment",
            DesignerTab::Sql => "design-tab-sql",
        }
    }
}

/// One control of the designer's per-column detail panel (the strip under the field list), which
/// describes the selected column.
///
/// The controls are one shared vocabulary: an engine lists the ones it has, in the order it wants
/// them. MySQL/MariaDB declare 自动递增 / 无符号 / 填充零; SQL Server declares 排序规则 / 稀疏; an
/// engine that has none of them still always gets 默认.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesignerColumnOption {
    /// The column's default value. A text field, and the one control every engine shows.
    Default,
    /// Auto-increment (MySQL) / identity (SQL Server, Oracle, PostgreSQL).
    AutoIncrement,
    /// How an identity column is generated: `ALWAYS` or `BY DEFAULT` (PostgreSQL).
    IdentityGeneration,
    /// The identity sequence's start value (PostgreSQL).
    IdentityStart,
    /// The identity sequence's increment (PostgreSQL).
    IdentityIncrement,
    /// Unsigned integer columns (MySQL/MariaDB).
    Unsigned,
    /// Zerofill (MySQL/MariaDB).
    Zerofill,
    /// The column's own collation, picked from the engine's list.
    Collation,
    /// Sparse columns (SQL Server).
    Sparse,
    /// The member values list for `ENUM` / `SET` columns (MySQL/MariaDB).
    EnumValues,
}

impl DesignerColumnOption {
    /// Whether the control is a check box; the rest ([`Default`](Self::Default),
    /// [`EnumValues`](Self::EnumValues) and [`Collation`](Self::Collation)) are editors.
    pub fn is_toggle(self) -> bool {
        match self {
            DesignerColumnOption::Default
            | DesignerColumnOption::EnumValues
            | DesignerColumnOption::Collation
            | DesignerColumnOption::IdentityGeneration
            | DesignerColumnOption::IdentityStart
            | DesignerColumnOption::IdentityIncrement => false,
            DesignerColumnOption::AutoIncrement
            | DesignerColumnOption::Unsigned
            | DesignerColumnOption::Zerofill
            | DesignerColumnOption::Sparse => true,
        }
    }

    /// i18n key of the control's label.
    pub fn label_key(self) -> &'static str {
        match self {
            DesignerColumnOption::Default => "design.default",
            DesignerColumnOption::EnumValues => "design.enum_values",
            DesignerColumnOption::AutoIncrement => "design.auto_increment",
            DesignerColumnOption::IdentityGeneration => "design.identity.generation",
            DesignerColumnOption::IdentityStart => "design.identity.start",
            DesignerColumnOption::IdentityIncrement => "design.identity.increment",
            DesignerColumnOption::Unsigned => "design.unsigned",
            DesignerColumnOption::Zerofill => "design.fill_zero",
            DesignerColumnOption::Collation => "design.collation",
            DesignerColumnOption::Sparse => "design.sparse",
        }
    }
}

/// One column of the designer's field list, after the name and type cells every engine has.
///
/// The variants are one vocabulary of *cells* (not of columns): `Length` and `Size` are the same
/// cell under the two engines' names, as are `Decimals`/`Scale` and `Generated`/`Computed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesignerFieldColumn {
    /// The display length (MySQL's 长度).
    Length,
    /// The display length under SQL Server's name (大小).
    Size,
    /// The decimal places (小数位).
    Decimals,
    /// The decimal places under SQL Server's name (比例).
    Scale,
    /// Whether the column is `NOT NULL`.
    NotNull,
    /// A generated-column expression (MySQL's 虚拟).
    Generated,
    /// A computed-column expression under SQL Server's name (计算列).
    Computed,
    /// Whether the column is part of the primary key.
    Key,
    /// The column's comment (备注).
    Comment,
    /// The column's collation.
    Collation,
    /// Whether the column is sparse.
    Sparse,
}

impl DesignerFieldColumn {
    /// i18n key of the cell's header label.
    pub fn label_key(self) -> &'static str {
        match self {
            DesignerFieldColumn::Length => "design.col.length",
            DesignerFieldColumn::Size => "design.col.size",
            DesignerFieldColumn::Decimals => "design.col.decimals",
            DesignerFieldColumn::Scale => "design.col.scale",
            DesignerFieldColumn::NotNull => "design.col.not_null",
            DesignerFieldColumn::Generated => "design.col.virtual",
            DesignerFieldColumn::Computed => "design.col.computed",
            DesignerFieldColumn::Key => "design.col.key",
            DesignerFieldColumn::Comment => "design.col.comment",
            DesignerFieldColumn::Collation => "design.col.collation",
            DesignerFieldColumn::Sparse => "design.col.sparse",
        }
    }
}

/// One row of the designer's Options tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DesignerOptionField {
    /// The storage engine (MySQL/MariaDB).
    Engine,
    /// The table's character set.
    Charset,
    /// The table's collation.
    Collation,
    /// The next auto-increment value.
    AutoIncrement,
    /// The table's comment.
    Comment,
}

impl DesignerOptionField {
    /// i18n key of the row's label.
    pub fn label_key(self) -> &'static str {
        match self {
            DesignerOptionField::Engine => "design.options.engine",
            DesignerOptionField::Charset => "design.options.charset",
            DesignerOptionField::Collation => "design.options.collation",
            DesignerOptionField::AutoIncrement => "design.options.auto_increment",
            DesignerOptionField::Comment => "design.comment_label",
        }
    }
}

/// Which columns an engine accepts an auto-increment (identity) on.
///
/// Every engine that has one restricts it — SQL Server and Oracle allow a single identity column per
/// table and only on a numeric type, MySQL's `AUTO_INCREMENT` must be an integer column, SQLite's
/// `AUTOINCREMENT` only exists on `INTEGER PRIMARY KEY` — so the designer asks this before offering
/// the control instead of showing it on every column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoIncrementRule {
    /// The base types the engine accepts, compared case-insensitively against the column's type with
    /// any `(…)` qualifier removed. An empty list accepts any type.
    pub types: Vec<&'static str>,
    /// Whether the engine allows at most one auto-increment column per table.
    pub one_per_table: bool,
    /// Whether an auto-increment can be added to (or removed from) a column the table already has.
    /// SQL Server cannot: `IDENTITY` is only accepted on a new column, so the designer offers the
    /// control on a freshly added column or a new table and shows it read-only otherwise.
    pub change_existing: bool,
}

impl Default for AutoIncrementRule {
    /// An unrestricted engine: any type, one per table, changeable in place.
    fn default() -> Self {
        Self {
            types: Vec::new(),
            one_per_table: true,
            change_existing: true,
        }
    }
}

impl AutoIncrementRule {
    /// Whether a column of type `data_type` may carry the engine's auto-increment.
    pub fn allows(&self, data_type: &str) -> bool {
        if self.types.is_empty() {
            return true;
        }
        let base = data_type
            .trim()
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        self.types.iter().any(|allowed| *allowed == base)
    }
}

/// How the designer's trigger body editor is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerDefinition {
    /// MySQL/MariaDB: the editor holds the trigger body; the designer writes
    /// `CREATE TRIGGER … FOR EACH ROW <body>` and edits timing/event separately.
    #[default]
    Body,
    /// SQL Server/PostgreSQL/Oracle: the editor holds the complete `CREATE TRIGGER` statement;
    /// the designer only drops and replays it, and the name/timing/event cells are read-only.
    Full,
}

/// One per-column option an engine's indexes carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesignerIndexColumn {
    /// The column's sort order (`ASC` / `DESC`); every B-tree engine has it.
    Sort,
    /// Index only a prefix of the column (MySQL/MariaDB's 子部分).
    PrefixLength,
}

impl DesignerIndexColumn {
    /// i18n key of the option's column header.
    pub fn label_key(self) -> &'static str {
        match self {
            DesignerIndexColumn::Sort => "design.index.column.sort",
            DesignerIndexColumn::PrefixLength => "design.index.column.prefix",
        }
    }
}

/// One choice of the designer's 索引类型 (index type) cell.
///
/// The vocabulary is each engine's own: `label` is the text that engine writes for the kind
/// (`NORMAL`, `FULLTEXT`, `CLUSTERED`, `BITMAP`, ...) and is drawn verbatim — an index type is the
/// engine's identifier, not a translatable UI string — so every engine's 索引类型 list is visibly
/// its own rather than MySQL's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesignerIndexKind {
    /// The text the 索引类型 cell shows and its drop-down offers, in the engine's own words.
    pub label: &'static str,
    /// Whether the kind makes the index `UNIQUE`.
    pub unique: bool,
    /// The `IndexDef::index_type` values the kind covers, its first entry being the value stored
    /// when the kind is picked. More than one entry means the engine's 索引方法 cell refines them
    /// (MySQL/MariaDB's `NORMAL`/`UNIQUE` cover `BTREE` and `HASH`); `None` means the engine stores
    /// no type of its own, so uniqueness alone decides the kind (PostgreSQL).
    pub types: Option<&'static [&'static str]>,
}

impl DesignerIndexKind {
    /// The value stored in `IndexDef::index_type` when the kind is picked.
    pub fn value(self) -> &'static str {
        self.types
            .and_then(|types| types.first().copied())
            .unwrap_or_default()
    }
}

/// One common default expression offered beside the designer's 默认 field.
///
/// The designer filters the list by the selected column's type, so a `varchar` column is not
/// offered `CURRENT_TIMESTAMP` and a `uuid` column is not offered `0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultSuggestion {
    /// The expression inserted after `DEFAULT`.
    pub value: &'static str,
    /// The base column types it applies to (`varchar`, `timestamp`, ...). Empty means every type.
    pub types: Vec<&'static str>,
}

impl DefaultSuggestion {
    /// A suggestion every column type can use (`NULL`).
    pub fn every(value: &'static str) -> Self {
        Self {
            value,
            types: Vec::new(),
        }
    }

    /// A suggestion narrowed to the engine's own column types.
    pub fn for_types(value: &'static str, types: &[&'static str]) -> Self {
        Self {
            value,
            types: types.to_vec(),
        }
    }

    /// Whether the suggestion applies to `data_type`. The comparison ignores a `(p[,s])` qualifier
    /// and an array suffix, so `varchar(200)` and `integer[]` match `varchar` and `integer`.
    pub fn applies(&self, data_type: &str) -> bool {
        if self.types.is_empty() {
            return true;
        }
        let base = base_column_type(data_type);
        self.types
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case(&base))
    }
}

/// The base name of a column type (`character varying(50)` -> `character varying`,
/// `integer[]` -> `integer`).
fn base_column_type(data_type: &str) -> String {
    let base = data_type.split('(').next().unwrap_or_default().trim();
    base.strip_suffix("[]").unwrap_or(base).trim().to_string()
}

/// How the table designer presents one engine's tables.
///
/// The designer is one view shared by every engine; this spec is what makes it engine-correct.
/// The default is MySQL's shape (the designer's historical behavior), so a driver that does not
/// opt in keeps working.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDesignerSpec {
    /// The sub-tabs shown, in order. An engine adds 触发器 when its driver can load and save
    /// triggers; the shared vocabulary includes it, but a driver that does not implement trigger
    /// DDL simply leaves it out.
    pub tabs: Vec<DesignerTab>,
    /// The per-column detail controls shown, in order.
    pub column_options: Vec<DesignerColumnOption>,
    /// The field-list columns shown, after 列名 and 类型.
    pub field_columns: Vec<DesignerFieldColumn>,
    /// The rows shown by the Options tab, in order.
    pub option_fields: Vec<DesignerOptionField>,
    /// Common default expressions offered beside the 默认 field, in order. The designer filters
    /// them by the selected column's type; empty leaves the field a plain text box.
    pub default_suggestions: Vec<DefaultSuggestion>,
    /// The per-column options the index field editor shows, in order.
    pub index_columns: Vec<DesignerIndexColumn>,
    /// The 索引类型 choices, in order: each engine's own index kinds.
    pub index_kinds: Vec<DesignerIndexKind>,
    /// Whether the engine's indexes carry a comment (MySQL/MariaDB's `INDEX … COMMENT`); the index
    /// grid leaves its 注释 column out when they do not.
    pub index_comment: bool,
    /// Where the engine accepts an auto-increment (identity) column.
    pub auto_increment: AutoIncrementRule,
    /// How the 触发器 editor's SQL text is interpreted (body vs. complete definition).
    pub trigger_definition: TriggerDefinition,
}

impl TableDesignerSpec {
    /// Every tab except the ones no engine but SQL Server has (唯一键 / 检查), which a driver adds
    /// back when it has them. Handy for engines that keep 选项/注释 but take no column extras.
    pub fn standard_tabs() -> Vec<DesignerTab> {
        vec![
            DesignerTab::Fields,
            DesignerTab::Indexes,
            DesignerTab::ForeignKeys,
            DesignerTab::Options,
            DesignerTab::Comment,
            DesignerTab::Sql,
        ]
    }

    /// The access methods the index grid's 索引方法 cell offers, or `None` when the engine has none
    /// and the grid leaves that column out. The methods are one kind's own type list: MySQL/MariaDB
    /// `NORMAL`/`UNIQUE` cover `BTREE` and `HASH`.
    pub fn index_methods(&self) -> Option<&'static [&'static str]> {
        self.index_kinds
            .iter()
            .filter_map(|kind| kind.types)
            .find(|types| types.len() > 1)
    }

    /// The kind one index row shows: the declared kind whose uniqueness and type match the index.
    /// `None` when the engine declares no such combination — the row then keeps the engine's own
    /// `index_type` text (Oracle's `FUNCTION-BASED NORMAL`, SQL Server's columnstore indexes, ...).
    pub fn index_kind(&self, index: &IndexDef) -> Option<&DesignerIndexKind> {
        let index_type = index.index_type.trim();
        self.index_kinds
            .iter()
            .find(|kind| {
                kind.unique == index.unique
                    && kind.types.is_none_or(|types| {
                        types.iter().any(|t| t.eq_ignore_ascii_case(index_type))
                    })
            })
            // An index with no type at all (a model that never stored one) still shows its kind,
            // decided by uniqueness, rather than an empty cell.
            .or_else(|| {
                index_type
                    .is_empty()
                    .then(|| {
                        self.index_kinds
                            .iter()
                            .find(|kind| kind.unique == index.unique)
                    })
                    .flatten()
            })
    }

    /// The 索引方法 value one index row shows: the matched kind's own type list when it has more
    /// than one entry (`BTREE`/`HASH`), with the index's stored type as the current one. `None` for
    /// a kind the cell does not refine.
    pub fn index_method(&self, index: &IndexDef) -> Option<&'static str> {
        let types = self.index_kind(index)?.types?;
        if types.len() < 2 {
            return None;
        }
        let index_type = index.index_type.trim();
        types
            .iter()
            .copied()
            .find(|t| t.eq_ignore_ascii_case(index_type))
            .or_else(|| types.first().copied())
    }
}

impl Default for TableDesignerSpec {
    /// MySQL/MariaDB's base shape: every standard tab, no unique-constraint tab (MySQL reports
    /// uniqueness through 索引), MySQL's three column extras, and MySQL's field-list columns.
    /// MySQL/MariaDB also have a 检查 tab since MySQL 8.0.16 / MariaDB 10.2, which those two
    /// drivers add themselves; the generic ODBC driver keeps this base unchanged.
    fn default() -> Self {
        Self {
            tabs: TableDesignerSpec::standard_tabs(),
            column_options: vec![
                DesignerColumnOption::Default,
                DesignerColumnOption::EnumValues,
                DesignerColumnOption::AutoIncrement,
                DesignerColumnOption::Unsigned,
                DesignerColumnOption::Zerofill,
            ],
            field_columns: vec![
                DesignerFieldColumn::Length,
                DesignerFieldColumn::Decimals,
                DesignerFieldColumn::NotNull,
                DesignerFieldColumn::Generated,
                DesignerFieldColumn::Key,
                DesignerFieldColumn::Comment,
            ],
            option_fields: vec![
                DesignerOptionField::Engine,
                DesignerOptionField::Charset,
                DesignerOptionField::Collation,
                DesignerOptionField::AutoIncrement,
                DesignerOptionField::Comment,
            ],
            default_suggestions: vec![
                DefaultSuggestion::for_types("CURRENT_TIMESTAMP", &["datetime", "timestamp"]),
                DefaultSuggestion::for_types("CURRENT_DATE", &["date"]),
                DefaultSuggestion::for_types("CURRENT_TIME", &["time"]),
                DefaultSuggestion::for_types(
                    "0",
                    &[
                        "tinyint",
                        "smallint",
                        "mediumint",
                        "int",
                        "integer",
                        "bigint",
                        "decimal",
                        "numeric",
                        "float",
                        "double",
                        "real",
                    ],
                ),
                DefaultSuggestion::for_types(
                    "''",
                    &[
                        "char",
                        "varchar",
                        "tinytext",
                        "text",
                        "mediumtext",
                        "longtext",
                        "enum",
                        "set",
                    ],
                ),
                DefaultSuggestion::for_types("true", &["boolean", "bool", "tinyint"]),
                DefaultSuggestion::for_types("false", &["boolean", "bool", "tinyint"]),
                DefaultSuggestion::every("NULL"),
            ],
            // MySQL's `AUTO_INCREMENT` is a single integer column per table, changeable in place.
            auto_increment: AutoIncrementRule {
                types: vec![
                    "tinyint",
                    "smallint",
                    "mediumint",
                    "int",
                    "integer",
                    "bigint",
                ],
                ..AutoIncrementRule::default()
            },
            // MySQL indexes sort per column and can index a fixed prefix of one (子部分).
            index_columns: vec![DesignerIndexColumn::Sort, DesignerIndexColumn::PrefixLength],
            // MySQL's own kinds: a plain or unique index is a B-tree (or hash) index, and
            // FULLTEXT/SPATIAL are kinds of their own.
            index_kinds: vec![
                DesignerIndexKind {
                    label: "NORMAL",
                    unique: false,
                    types: Some(&["BTREE", "HASH"]),
                },
                DesignerIndexKind {
                    label: "UNIQUE",
                    unique: true,
                    types: Some(&["BTREE", "HASH"]),
                },
                DesignerIndexKind {
                    label: "FULLTEXT",
                    unique: false,
                    types: Some(&["FULLTEXT"]),
                },
                DesignerIndexKind {
                    label: "SPATIAL",
                    unique: false,
                    types: Some(&["SPATIAL"]),
                },
            ],
            // `INDEX … COMMENT 'x'` is MySQL's; no other engine the designer has a driver for
            // stores an index comment.
            index_comment: true,
            // MySQL/MariaDB's trigger editor edits the body; `CREATE TRIGGER` is written by the
            // driver from the name/timing/event cells.
            trigger_definition: TriggerDefinition::Body,
        }
    }
}

/// Presentation and capability metadata for one driver.
#[derive(Debug, Clone)]
pub struct DriverDescriptor {
    /// The engine id (e.g. `sqlserver`).
    pub id: DriverId,
    /// The user-facing engine name (e.g. `SQL Server`).
    pub display_name: String,
    /// The port pre-filled by the connection form.
    pub default_port: u16,
    /// Whether the engine connects to a file rather than a network server.
    pub is_file_based: bool,
    /// The embedded asset path of the engine's icon (e.g. `icons/sqlserver.svg`).
    pub icon: &'static str,
    /// How the icon is drawn in the connection tree.
    pub icon_style: DriverIconStyle,
    /// The capabilities the engine advertises to the UI.
    pub capabilities: DriverCapabilities,
    /// Which fields and tabs the New/Edit Database dialog shows for this engine.
    pub database_editor: DatabaseEditorSpec,
    /// Which extra fields the New/Edit Connection form shows for this engine.
    pub connection_form: ConnectionFormSpec,
    /// How the table designer presents this engine's tables.
    pub table_designer: TableDesignerSpec,
    /// Sort key for the New Connection menu (lower first); ties fall back to the display name.
    pub order: u16,
}

/// Whether a column type carries a member list (`ENUM('a','b')`, `SET('x','y')`).
pub fn is_member_list_type(data_type: &str) -> bool {
    matches!(
        data_type.trim().to_ascii_lowercase().as_str(),
        "enum" | "set"
    )
}

/// Whether a column type is numeric (integers, floating point, decimal). Listed explicitly rather
/// than by suffix: `point`/`multipoint` are geometry types that a suffix test would read as
/// integers.
pub fn is_numeric_type(data_type: &str) -> bool {
    let data_type = data_type.trim().to_ascii_lowercase();
    matches!(
        data_type.as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "serial"
            | "decimal"
            | "numeric"
            | "dec"
            | "fixed"
            | "float"
            | "double"
            | "real"
            | "bit"
            | "year"
    )
}

/// The character a MySQL string literal's backslash escape stands for. Unknown escapes are the
/// escaped character itself, exactly as the server reads them.
fn unescape_member_char(escaped: char) -> char {
    match escaped {
        '0' => '\0',
        'b' => '\u{8}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'Z' => '\u{1a}',
        _ => escaped,
    }
}

/// One member value as it appears inside a MySQL string literal, so a value that carries a quote,
/// a backslash or a control character survives the round trip through the member list.
fn escape_member_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            // The SQL-standard doubling works even with `NO_BACKSLASH_ESCAPES`.
            '\'' => out.push_str("''"),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\0"),
            '\u{8}' => out.push_str("\\b"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{1a}' => out.push_str("\\Z"),
            other => out.push(other),
        }
    }
    out
}

/// Parse a raw member list string (e.g. `'a','b','c'` or `a,b,c`) into individual values.
pub fn parse_member_list(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if trimmed.contains('\'') {
        let mut members = Vec::new();
        let mut chars = trimmed.chars().peekable();
        while let Some(&ch) = chars.peek() {
            if ch.is_whitespace() || ch == ',' {
                chars.next();
                continue;
            }
            if ch == '\'' {
                chars.next();
                let mut current = String::new();
                while let Some(c) = chars.next() {
                    if c == '\'' {
                        if chars.peek() == Some(&'\'') {
                            chars.next();
                            current.push('\'');
                        } else {
                            break;
                        }
                    } else if c == '\\' {
                        if let Some(&next) = chars.peek() {
                            chars.next();
                            // `\%` / `\_` keep their backslash even outside a `LIKE` pattern.
                            if matches!(next, '%' | '_') {
                                current.push('\\');
                            }
                            current.push(unescape_member_char(next));
                        } else {
                            current.push('\\');
                        }
                    } else {
                        current.push(c);
                    }
                }
                members.push(current);
            } else {
                let mut current = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ',' {
                        break;
                    }
                    current.push(c);
                    chars.next();
                }
                let t = current.trim();
                if !t.is_empty() {
                    members.push(t.to_string());
                }
            }
        }
        members
    } else {
        trimmed
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Format individual member values into a SQL-safe member list string (e.g. `'a','b','c'`).
pub fn format_member_list(members: &[String]) -> String {
    members
        .iter()
        .map(|member| format!("'{}'", escape_member_value(member)))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_list_parsing_and_formatting() {
        assert!(is_member_list_type("enum"));
        assert!(is_member_list_type("ENUM"));
        assert!(is_member_list_type("set"));
        assert!(!is_member_list_type("varchar"));
        assert!(!is_member_list_type("int"));

        assert!(is_numeric_type("int"));
        assert!(is_numeric_type("BIGINT"));
        assert!(is_numeric_type("decimal"));
        assert!(!is_numeric_type("varchar"));
        assert!(!is_numeric_type("enum"));

        let items = parse_member_list("'high','slow'");
        assert_eq!(items, vec!["high", "slow"]);
        assert_eq!(format_member_list(&items), "'high','slow'");

        let escaped = parse_member_list("'it''s','ok'");
        assert_eq!(escaped, vec!["it's", "ok"]);
        assert_eq!(format_member_list(&escaped), "'it''s','ok'");

        let unquoted = parse_member_list("male, female");
        assert_eq!(unquoted, vec!["male", "female"]);
        assert_eq!(format_member_list(&unquoted), "'male','female'");

        assert!(parse_member_list("").is_empty());
    }

    #[test]
    fn member_list_escapes_survive_a_round_trip() {
        // A backslash must be written back doubled, or MySQL reads the pair as an escape.
        let backslash = parse_member_list(r"'C:\\tmp','a\\b'");
        assert_eq!(backslash, vec!["C:\\tmp", "a\\b"]);
        assert_eq!(format_member_list(&backslash), r"'C:\\tmp','a\\b'");
        assert_eq!(
            parse_member_list(&format_member_list(&backslash)),
            backslash
        );

        // The escape sequences MySQL itself writes in SHOW CREATE come back as their characters.
        let control = parse_member_list(r"'a\nb','it\'s','\Z'");
        assert_eq!(control, vec!["a\nb", "it's", "\u{1a}"]);
        assert_eq!(format_member_list(&control), r"'a\nb','it''s','\Z'");

        // `\%` / `\_` keep their backslash in a string literal, so they do not lose it.
        let pattern = parse_member_list(r"'\%','\_'");
        assert_eq!(pattern, vec!["\\%", "\\_"]);
        assert_eq!(format_member_list(&pattern), r"'\\%','\\_'");
    }

    #[test]
    fn auto_increment_matches_the_type_without_its_qualifier() {
        let rule = AutoIncrementRule {
            types: vec!["int", "bigint"],
            ..AutoIncrementRule::default()
        };
        assert!(rule.allows("int"));
        assert!(rule.allows("INT"));
        assert!(rule.allows(" int(11) "));
        assert!(rule.allows("bigint"));
        assert!(!rule.allows("nvarchar"));
        assert!(!rule.allows("decimal(10,2)"));
    }

    #[test]
    fn an_engine_without_a_type_list_accepts_any_column() {
        // The default is unrestricted in type, one per table, and changeable in place.
        let rule = AutoIncrementRule::default();
        assert!(rule.allows("nvarchar"));
        assert!(rule.one_per_table);
        assert!(rule.change_existing);
    }

    #[test]
    fn default_suggestions_follow_the_column_type() {
        let spec = TableDesignerSpec::default();
        let values = |data_type: &str| {
            spec.default_suggestions
                .iter()
                .filter(|suggestion| suggestion.applies(data_type))
                .map(|suggestion| suggestion.value)
                .collect::<Vec<_>>()
        };
        // A text column gets an empty string and the universal NULL, not timestamp functions.
        assert_eq!(values("varchar(200)"), vec!["''", "NULL"]);
        assert_eq!(values("text"), vec!["''", "NULL"]);
        // Numeric types get 0 (tinyint also carries its boolean spellings).
        assert_eq!(values("bigint"), vec!["0", "NULL"]);
        assert_eq!(values("tinyint"), vec!["0", "true", "false", "NULL"]);
        // The qualifier and an array suffix are ignored.
        assert_eq!(values("integer(11)"), vec!["0", "NULL"]);
        assert_eq!(values("integer[]"), vec!["0", "NULL"]);
        // A timestamp gets the timestamp expression only.
        assert_eq!(values("timestamp"), vec!["CURRENT_TIMESTAMP", "NULL"]);
    }

    #[test]
    fn the_default_designer_offers_mysqls_shape() {
        let spec = TableDesignerSpec::default();
        assert!(spec.tabs.contains(&DesignerTab::Fields));
        assert!(!spec.tabs.contains(&DesignerTab::UniqueKeys));
        assert!(
            spec.column_options
                .contains(&DesignerColumnOption::Unsigned)
        );
        assert!(
            spec.column_options
                .contains(&DesignerColumnOption::Zerofill)
        );
        assert!(!spec.column_options.contains(&DesignerColumnOption::Sparse));
        assert!(spec.auto_increment.allows("int"));
        assert!(!spec.auto_increment.allows("varchar"));
    }

    #[test]
    fn mysqls_kinds_round_trip_through_the_model() {
        let spec = TableDesignerSpec::default();
        let index = |unique, index_type: &str| IndexDef {
            unique,
            index_type: index_type.to_string(),
            ..IndexDef::default()
        };

        // A plain and a unique index share the B-tree/hash method list; the kind tells them apart.
        assert_eq!(
            spec.index_kind(&index(false, "BTREE")).unwrap().label,
            "NORMAL"
        );
        assert_eq!(
            spec.index_kind(&index(true, "BTREE")).unwrap().label,
            "UNIQUE"
        );
        // The 索引方法 cell refines NORMAL/UNIQUE, so a HASH index is still its kind.
        assert_eq!(
            spec.index_kind(&index(false, "HASH")).unwrap().label,
            "NORMAL"
        );
        assert_eq!(spec.index_method(&index(false, "HASH")), Some("HASH"));
        assert_eq!(spec.index_method(&index(true, "")), Some("BTREE"));
        // FULLTEXT/SPATIAL are kinds of their own and carry no method.
        assert_eq!(
            spec.index_kind(&index(false, "FULLTEXT")).unwrap().label,
            "FULLTEXT"
        );
        assert_eq!(spec.index_method(&index(false, "FULLTEXT")), None);
        assert_eq!(spec.index_methods(), Some(&["BTREE", "HASH"][..]));
        assert!(spec.index_comment);
    }

    #[test]
    fn an_engine_without_types_keeps_its_kinds_to_uniqueness() {
        // PostgreSQL stores no index type of its own, so NORMAL/UNIQUE are decided by uniqueness
        // alone and there is no 索引方法 column.
        let spec = TableDesignerSpec {
            index_kinds: vec![
                DesignerIndexKind {
                    label: "NORMAL",
                    unique: false,
                    types: None,
                },
                DesignerIndexKind {
                    label: "UNIQUE",
                    unique: true,
                    types: None,
                },
            ],
            index_comment: false,
            ..TableDesignerSpec::default()
        };
        assert_eq!(
            spec.index_kind(&IndexDef::default()).unwrap().label,
            "NORMAL"
        );
        assert!(
            spec.index_kind(&IndexDef {
                unique: true,
                ..IndexDef::default()
            })
            .is_some()
        );
        assert_eq!(spec.index_methods(), None);
        assert!(
            spec.index_kind(&IndexDef::default())
                .unwrap()
                .value()
                .is_empty()
        );
    }
}
