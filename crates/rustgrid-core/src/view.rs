//! Engine-agnostic models for database views, backing the Views main tab and its designer. The UI
//! only ever sees these types; each driver maps them to its own catalog and DDL syntax.

use serde::{Deserialize, Serialize};

/// One view as listed by the Views tab and shown by the view designer's 高级 page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ViewInfo {
    pub name: String,
    /// The `DEFINER` account, e.g. `root@localhost`.
    #[serde(default)]
    pub definer: String,
    /// `DEFINER` or `INVOKER`.
    #[serde(default)]
    pub security_type: String,
    /// `UNDEFINED`, `MERGE` or `TEMPTABLE`.
    #[serde(default)]
    pub algorithm: String,
    /// `NONE`, `LOCAL` or `CASCADED`.
    #[serde(default)]
    pub check_option: String,
    /// Whether the view is updatable (`IS_UPDATABLE = YES`).
    #[serde(default)]
    pub updatable: bool,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub modified: Option<String>,
}

impl ViewInfo {
    /// The label used by the view list and tab captions.
    pub fn label(&self) -> String {
        self.name.clone()
    }
}

/// The full definition and creation settings of one view.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewDetails {
    pub info: ViewInfo,
    /// The full `CREATE ... VIEW` statement as the engine reports it (from `SHOW CREATE VIEW`).
    pub definition: String,
    pub character_set_client: String,
    pub collation_connection: String,
}

/// The edit the view designer applies on Save: the full `CREATE` statement plus the identity the
/// engine needs to drop/replace the view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewEdit {
    pub name: String,
    /// The full `CREATE ... VIEW` statement to run.
    pub definition: String,
}
