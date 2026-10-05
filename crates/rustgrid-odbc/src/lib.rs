//! A generic ODBC driver for RustGrid.
//!
//! This is the **optional** fallback for engines RustGrid has no native Rust driver for (Oracle,
//! DB2, 达梦, ...): it connects through whatever ODBC driver the user has installed on the system
//! and speaks the engine's SQL directly.
//!
//! Unlike a link-time ODBC binding, the driver manager (`odbc32.dll` / `libodbc.so`) is loaded
//! **at runtime** through [`api`]. A build therefore carries **no link-time dependency** on ODBC,
//! and a machine without an ODBC driver manager still starts normally — the ODBC engine simply
//! reports that no drivers are available. This mirrors how Navicat offers ODBC as a plain option.

mod api;
mod connection;
mod driver;

pub use connection::OdbcConnection;
pub use driver::OdbcDriver;
