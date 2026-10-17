# sqlx-mysql 0.9.0, patched for RustGrid

This is the published `sqlx-mysql` 0.9.0 crate (crates.io checksum
`90b8020fe17c5f2c245bfa2505d7ef59c5604839527c740266ad2214acebea27`), wired in through
`[patch.crates-io]` in the root `Cargo.toml`. The first commit that added this directory imports
it verbatim; every change since is listed here. Licence: MIT OR Apache-2.0 (the files alongside).

## Changes

1. **Login to servers without pluggable authentication** (`src/connection/establish.rs`).
   MySQL before 5.5.7 names no auth plugin in its handshake. Upstream then sends an empty auth
   response even when a password is set, and the server answers
   `Access denied ... (using password: NO)`. When the server supports
   `CLIENT_SECURE_CONNECTION` (4.1+), the handshake now falls back to the `mysql_native_password`
   scramble those servers expect.
2. **A clear error for pre-4.1 password hashes** (same file). A lone `0xfe` reply is the server
   asking for the insecure `mysql_old_password` scheme; it is reported as such instead of as an
   undecodable auth-switch packet.
3. **`utf8` instead of `utf8mb4` on servers older than 5.5** (`src/options/connect.rs`). The
   post-connect `SET NAMES utf8mb4` fails there with "Unknown character set". Only the default
   charset is replaced, only when no collation was chosen, and only by major.minor (MariaDB 10+
   reports `5.5.5-10.x.y-MariaDB`).
4. A decode test for a MySQL 5.1-style handshake (`src/protocol/connect/handshake.rs`).

## Removing it

Once an sqlx release contains equivalent fixes, delete this directory, the `[patch.crates-io]`
entry and the `exclude` line in the root `Cargo.toml`, and run `cargo update -p sqlx-mysql`.
