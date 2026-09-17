use std::collections::HashMap;
use std::sync::Arc;

use crate::driver::Driver;
use crate::model::DriverId;

pub trait DriverSource: Send + Sync {
    fn drivers(&self) -> Vec<Arc<dyn Driver>>;
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

    pub fn len(&self) -> usize {
        self.drivers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}
