//! Engine capability flags shared by every driver.
//!
//! A driver advertises what its engine can do through [`DriverCapabilities`]; the UI decides
//! which actions to show or disable by asking for a capability rather than matching on the engine
//! id. This is the structural replacement for the historical `supports_*` booleans, which remain
//! available and feed the default [`DriverCapabilities`](crate::DriverDescriptor) construction.

/// One engine capability that some drivers lack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DriverCapability {
    /// Create/alter/drop databases.
    DatabaseManagement,
    /// Server accounts and their privileges.
    Users,
    /// Stored routines (functions and procedures).
    Routines,
    /// Schemas as a first-class object.
    Schemas,
    /// Scheduled events.
    Events,
    /// Views.
    Views,
}

/// The set of capabilities an engine supports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriverCapabilities {
    enabled: Vec<DriverCapability>,
}

impl DriverCapabilities {
    /// An empty set.
    pub fn none() -> Self {
        Self::default()
    }

    /// Builds the set from the historical boolean flags a [`Driver`](crate::Driver) exposes.
    pub fn from_flags(
        database_management: bool,
        users: bool,
        routines: bool,
        schemas: bool,
    ) -> Self {
        Self::none()
            .with_flag(database_management, DriverCapability::DatabaseManagement)
            .with_flag(users, DriverCapability::Users)
            .with_flag(routines, DriverCapability::Routines)
            .with_flag(schemas, DriverCapability::Schemas)
    }

    fn with_flag(mut self, enabled: bool, capability: DriverCapability) -> Self {
        if enabled {
            self = self.with(capability);
        }
        self
    }

    /// Returns the set with `capability` added (idempotent).
    pub fn with(mut self, capability: DriverCapability) -> Self {
        if !self.enabled.contains(&capability) {
            self.enabled.push(capability);
        }
        self
    }

    /// Whether the engine supports `capability`.
    pub fn has(&self, capability: DriverCapability) -> bool {
        self.enabled.contains(&capability)
    }

    /// Every capability in the set.
    pub fn iter(&self) -> impl Iterator<Item = DriverCapability> + '_ {
        self.enabled.iter().copied()
    }
}
