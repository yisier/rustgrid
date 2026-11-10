# libsqlite3-sys 0.37.0, patched for RustGrid

This is the API surface of the published `libsqlite3-sys` 0.37.0 crate (crates.io checksum
`b1f111c8c41e7c61c49cd34e44c7619462967221a6443b0ec299e0ac30cfb9b1`): `src/error.rs` is copied
verbatim, `src/lib.rs` is the same file with `include!` pointed at the local bindings (and the
unused OpenSSL `extern crate` removed), and `src/bindings.rs` is the crate's SQLCipher bindings
(`sqlcipher/bindgen_bundled_version.rs`). It is wired in through `[patch.crates-io]` in the root
`Cargo.toml`.

## Why

`sqlx-sqlite` 0.9 needs a `libsqlite3-sys` below 0.38, whose `bundled-sqlcipher` feature links
OpenSSL. On Windows that requires a system OpenSSL or a vendored OpenSSL build (Perl, NASM,
`make`), which the project does not otherwise need. SQLCipher's amalgamation also has a libtomcrypt
provider, and the `sqlcipher-src` crate ships it as C sources, so this crate builds SQLCipher with
libtomcrypt on every platform and adds no native build tooling.

## Changes

1. `build.rs` compiles `sqlcipher-src`'s SQLCipher amalgamation and its libtomcrypt provider
   (AES/CBC, SHA-1/256/512, HMAC, PBKDF2, Fortuna) instead of SQLite/OpenSSL.
2. `src/bindings.rs` is included directly instead of being copied from `OUT_DIR` by the build
   script. `Cargo.toml` keeps only the features the workspace's dependency graph asks for.

## Removing it

Once `libsqlite3-sys` builds SQLCipher with libtomcrypt upstream (or sqlx allows 0.39+), delete
this directory, the `[patch.crates-io]` entry and the `exclude` line in the root `Cargo.toml`, and
run `cargo update -p libsqlite3-sys`.
