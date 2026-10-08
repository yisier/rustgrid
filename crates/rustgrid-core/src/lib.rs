pub mod capability;
pub mod descriptor;
pub mod dialect;
pub mod driver;
pub mod error;
pub mod model;
pub mod options;
pub mod registry;
pub mod routine;
pub mod user;
pub mod view;

pub use capability::{DriverCapabilities, DriverCapability};
pub use descriptor::{ConnectionFormSpec, DriverDescriptor, DriverIconStyle};
pub use dialect::DriverDialect;
pub use driver::{Connection, DatabaseEditorSpec, DatabaseEditorTab, Driver, UserEditorSpec};
pub use error::{Error, Result};
pub use model::{
    BackupObjectKind, CellValue, ColumnDef, ColumnInfo, ConnectionConfig, ConnectionProfile,
    DatabaseInfo, DatabaseOptions, DriverId, FilterCondition, FilterConjunction, FilterGroup,
    FilterNode, FilterOperator, ForeignKeyDef, IndexDef, ObjectDump, ObjectKind, PageRequest,
    QueryResult, RowInsert, RowUpdate, SavedBackup, SavedBackupSelection, SavedQuery, SortColumn,
    TableInfo, TableOptions, TablePage, TableSchema, TableStatus, TriggerDef,
};
pub use options::{ConnectionOptions, TlsMode, TlsOptions, TunnelAuth, TunnelKind, TunnelLayer};
pub use registry::{BuiltinDriverSource, DriverRegistry, DriverSource};
pub use routine::{RoutineDetails, RoutineEdit, RoutineInfo, RoutineKind};
pub use user::{
    ObjectGrant, ObjectPrivilegeRow, PrivilegeCatalog, PrivilegeGroup, PrivilegeId, PrivilegeInfo,
    PrivilegePreset, PrivilegeScope, RoleMembership, UserAccount, UserDetails, UserEdit,
    UserEditSection,
};
pub use view::{ViewDetails, ViewEdit, ViewInfo};
