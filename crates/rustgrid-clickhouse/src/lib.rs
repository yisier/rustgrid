//! The ClickHouse driver.
//!
//! ClickHouse is RustGrid's OLAP engine. It is reached through the **HTTP interface** (port 8123,
//! or 8443 with TLS) with the official `clickhouse` crate, reusing the workspace's rustls stack
//! (no OpenSSL, no C toolchain) and the shared SSH/SOCKS5/HTTP tunnel.
//!
//! The driver is a normal [`rustgrid_core::Driver`]: the app browses databases, tables and views,
//! pages the data grid with the filter tree and sorting, exports through the existing wizard, and
//! runs SQL in the query editor. Two ClickHouse-specific choices stand out:
//!
//! - **The grid is read-only** (`DriverCapability::ReadOnlyGrid`). MergeTree tables have no unique
//!   constraint, so no column set identifies a row, and `ALTER TABLE … UPDATE/DELETE` is an
//!   asynchronous mutation. Writes are done in the SQL editor; the import wizard can still insert
//!   rows through [`rustgrid_core::Connection::insert_rows`].
//! - **Results are read as `JSONCompactEachRowWithNamesAndTypes`**, which reports each result
//!   set's column names and ClickHouse types together with its rows, so an arbitrary statement can
//!   be shown without a separate schema request.

mod connection;
mod driver;
mod helpers;

pub use driver::ClickHouseDriver;
