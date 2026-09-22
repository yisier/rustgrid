//! Engine-agnostic models for stored routines (functions and procedures), backing the Functions
//! main tab and its editor. The UI only ever sees these types; each driver maps them to its own
//! catalog and DDL syntax.

use serde::{Deserialize, Serialize};

/// Whether a stored routine is a function (returns a value) or a procedure (called for effect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutineKind {
    #[default]
    Function,
    Procedure,
}

impl RoutineKind {
    /// Every kind, in the order the UI presents them.
    pub const ALL: [RoutineKind; 2] = [RoutineKind::Function, RoutineKind::Procedure];

    /// The i18n key for the kind's label.
    pub fn label_key(self) -> &'static str {
        match self {
            RoutineKind::Function => "routine.kind.function",
            RoutineKind::Procedure => "routine.kind.procedure",
        }
    }

    /// The DDL keyword, as used in `CREATE <keyword>` / `DROP <keyword>`.
    pub fn sql_name(self) -> &'static str {
        match self {
            RoutineKind::Function => "FUNCTION",
            RoutineKind::Procedure => "PROCEDURE",
        }
    }

    pub fn is_procedure(self) -> bool {
        matches!(self, RoutineKind::Procedure)
    }

    /// Resolve a kind from a DDL keyword (case-insensitive).
    pub fn from_sql_name(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("PROCEDURE") {
            Some(RoutineKind::Procedure)
        } else if name.eq_ignore_ascii_case("FUNCTION") {
            Some(RoutineKind::Function)
        } else {
            None
        }
    }
}

/// One stored routine as listed by the Functions tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RoutineInfo {
    pub name: String,
    pub kind: RoutineKind,
    /// The routine's comment (`ROUTINE_COMMENT`).
    #[serde(default)]
    pub comment: String,
    /// The return type of a function (empty for a procedure).
    #[serde(default)]
    pub return_type: String,
    /// The `DEFINER` account, e.g. `root@localhost`.
    #[serde(default)]
    pub definer: String,
    #[serde(default)]
    pub deterministic: bool,
    /// `CONTAINS SQL`, `NO SQL`, `READS SQL DATA` or `MODIFIES SQL DATA`.
    #[serde(default)]
    pub data_access: String,
    /// `DEFINER` or `INVOKER`.
    #[serde(default)]
    pub security_type: String,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub modified: Option<String>,
}

impl RoutineInfo {
    /// The label used by the routine list and tab captions, e.g. `f (Function)`.
    pub fn label(&self) -> String {
        self.name.clone()
    }
}

/// The full definition and session settings of one stored routine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoutineDetails {
    pub info: RoutineInfo,
    /// The full `CREATE ...` statement as the engine reports it (from `SHOW CREATE`).
    pub definition: String,
    /// The `sql_mode` in effect when the routine was created.
    pub sql_mode: String,
    pub character_set_client: String,
    pub collation_connection: String,
    pub database_collation: String,
}

/// The edit the routine editor applies on Save: the full `CREATE` statement plus the identity the
/// engine needs to drop/replace the routine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineEdit {
    pub kind: RoutineKind,
    pub name: String,
    /// The full `CREATE ...` statement to run.
    pub definition: String,
}
