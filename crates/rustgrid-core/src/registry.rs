use std::collections::HashMap;
use std::sync::Arc;

use crate::driver::Driver;
use crate::model::DriverId;

pub trait DriverSource: Send + Sync {
    fn drivers(&self) -> Vec<Arc<dyn Driver>>;
}

/// The list of drivers compiled into this build.
///
/// This is the single `DriverSource` the app registers today. Swapping it (or adding another
/// source) is how a future build would contribute drivers from elsewhere without the UI changing.
#[derive(Default)]
pub struct BuiltinDriverSource {
    drivers: Vec<Arc<dyn Driver>>,
}

impl BuiltinDriverSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one driver to the source (builder style).
    pub fn with(mut self, driver: Arc<dyn Driver>) -> Self {
        self.drivers.push(driver);
        self
    }
}

impl DriverSource for BuiltinDriverSource {
    fn drivers(&self) -> Vec<Arc<dyn Driver>> {
        self.drivers.clone()
    }
}

#[derive(Default)]
pub struct DriverRegistry {
    drivers: HashMap<DriverId, Arc<dyn Driver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, driver: Arc<dyn Driver>) {
        self.drivers.insert(driver.id(), driver);
    }

    pub fn register_source(&mut self, source: &dyn DriverSource) {
        for driver in source.drivers() {
            self.register(driver);
        }
    }

    pub fn get(&self, id: &DriverId) -> Option<Arc<dyn Driver>> {
        self.drivers.get(id).cloned()
    }

    pub fn drivers(&self) -> Vec<Arc<dyn Driver>> {
        self.drivers.values().cloned().collect()
    }

    /// Every registered driver, ordered by its descriptor's menu `order` (ties broken by display
    /// name). Used by the New Connection menu so its order is deterministic and engine-driven.
    pub fn drivers_sorted(&self) -> Vec<Arc<dyn Driver>> {
        let mut drivers: Vec<Arc<dyn Driver>> = self.drivers.values().cloned().collect();
        drivers.sort_by(|a, b| {
            let (a, b) = (a.descriptor(), b.descriptor());
            a.order
                .cmp(&b.order)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        drivers
    }

    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}
