//! MongoDB support for RustGrid.
//!
//! A MongoDB connection is a document store: databases hold collections, a collection is shown as
//! a table (its columns are the sampled top-level BSON fields), one document is one grid row, and
//! `_id` is the key every edit and delete is addressed by. The query tab runs JSON commands
//! (`find`, `aggregate`, `count`, `distinct`, any `runCommand` document) rather than a JavaScript
//! shell, so it reuses the app's existing editor and result grid.

mod connection;
mod driver;
pub mod filter;
pub mod value;

pub use driver::{
    MongoDriver, OPT_AUTH_MECHANISM, OPT_AUTH_SOURCE, OPT_READ_PREFERENCE, OPT_REPLICA_SET,
};
