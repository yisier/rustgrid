//! The Oracle driver, built on Oracle's official pure-Rust thin driver `oracledb` (no OCI /
//! Instant Client).
//!
//! Oracle is a schema-first engine like SQL Server and PostgreSQL: one connection sees every
//! schema (user) of its service/PDB, so a single pool serves every operation. `oracledb` is a
//! synchronous API, so every trait method bridges into a blocking task (see `connection.rs`).

mod backup;
mod connection;
mod driver;
mod helpers;
mod routine;
mod schema;
mod user;
mod view;

pub use connection::OracleConnection;
pub use driver::OracleDriver;
