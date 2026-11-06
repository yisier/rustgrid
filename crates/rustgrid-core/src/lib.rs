pub mod capability;
pub mod descriptor;
pub mod dialect;
pub mod driver;
pub mod error;
pub mod model;
pub mod options;
pub mod portable;
pub mod registry;
pub mod routine;
pub mod split;
pub mod sync;
pub mod user;
pub mod view;

pub use capability::{DriverCapabilities, DriverCapability};
pub use descriptor::{
    AdvancedFields, AutoIncrementRule, ConnectionFieldChoice, ConnectionFieldKind,
    ConnectionFieldLabel, ConnectionFieldSpec, ConnectionFormSpec, ConnectionHomePage,
    ConnectionPage, ConnectionStandardField, DefaultSuggestion, DesignerColumnOption,
    DesignerFieldColumn, DesignerIndexColumn, DesignerIndexKind, DesignerOptionField, DesignerTab,
    DriverDescriptor, DriverIconStyle, TableDesignerSpec, TriggerDefinition,
};
pub use dialect::DriverDialect;
pub use driver::{
    Connection, DatabaseEditorSpec, DatabaseEditorTab, Driver, ScriptSession, UserEditorSpec,
    Watch, WatchEvent, WatchSpec,
};
pub use error::{Error, Result};
pub use model::{
    BackupObjectKind, CellValue, CheckDef, CollectionDefinition, ColumnDef, ColumnInfo,
    ConnectionConfig, ConnectionProfile, DatabaseInfo, DatabaseOptions, DriverId, ExcludeDef,
    FilterCondition, FilterConjunction, FilterGroup, FilterNode, FilterOperator, ForeignKeyDef,
    IndexColumnDef, IndexDef, ObjectCreate, ObjectDump, ObjectKind, ObjectNamespace, PageRequest,
    QueryResult, RowInsert, RowUpdate, RuleDef, SavedBackup, SavedBackupSelection, SavedQuery,
    SavedTransfer, SortColumn, TableInfo, TableOptions, TablePage, TableSchema, TableStatus,
    TransferMode, TriggerDef, UniqueKeyDef,
};
pub use options::{ConnectionOptions, TlsMode, TlsOptions, TunnelAuth, TunnelKind, TunnelLayer};
pub use portable::{LogicalType, map_schema, map_table_name};
pub use registry::{BuiltinDriverSource, DriverRegistry, DriverSource};
pub use routine::{RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
pub use sync::{
    CellChange, DataDiff, RowChange, RowChangeKind, SchemaChange, SchemaChangeKind,
    column_type_text, delete_row_change, diff_columns, diff_rows, diff_schema, insert_row_change,
    render_change_sql, render_literal, update_row_change, usable_key_columns,
};
pub use user::{
    AccountKind, DefaultObjectType, DefaultPrivilege, DefaultPrivilegeInfo, ObjectGrant,
    ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup, PrivilegeId, PrivilegeInfo,
    PrivilegePreset, PrivilegeScope, RoleMembership, SecurableClass, ServerSecurableGrant,
    UserAccount, UserDetails, UserEdit, UserEditSection, UserMapping,
};
pub use view::{ViewDetails, ViewEdit, ViewInfo};
