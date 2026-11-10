//! Builds SQLCipher + libtomcrypt from `sqlcipher-src`'s C sources.
//!
//! Upstream `libsqlite3-sys` only supports SQLCipher with OpenSSL (system or vendored), which
//! makes a portable Windows build depend on Perl/OpenSSL. SQLCipher's amalgamation also supports
//! libtomcrypt, and `sqlcipher-src` ships that provider as C sources, so this patched crate builds
//! SQLCipher with libtomcrypt on every platform. Plain SQLite databases still open: SQLCipher
//! only encrypts once a key is set.
//!
//! The resulting library replaces SQLite for the whole application, so the feature set mirrors
//! `libsqlite3-sys` 0.37's bundled build.

use std::env;

/// The minimal libtomcrypt configuration SQLCipher needs. Every SQLCipher and libtomcrypt
/// translation unit must see exactly the same switches: the `hmac_state`, `cbc_state` and
/// `prng_state` layouts depend on them.
///
/// `LTC_NOTHING` disables libtomcrypt's default "everything" configuration; the list below is
/// what `crypto_libtomcrypt.c` looks up (AES, CBC, SHA-1/256/512, HMAC, PBKDF2 and Fortuna).
const LTC_DEFINES: &[&str] = &[
    "LTC_NOTHING",
    "LTC_RIJNDAEL",
    "LTC_CBC_MODE",
    "LTC_SHA1",
    "LTC_SHA256",
    "LTC_SHA512",
    "LTC_HASH_HELPERS",
    "LTC_HMAC",
    "LTC_FORTUNA",
    "LTC_PKCS_5",
    "LTC_RNG_GET_BYTES",
    "LTC_DEVRANDOM",
    "LTC_TRY_URANDOM_FIRST",
    "LTC_NO_TEST",
    "LTC_NO_ASM",
    "LTC_NO_MATH",
    "LTC_CLEAN_STACK",
];

fn main() {
    let source_dir = sqlcipher_src::source_dir();
    let tomcrypt_include = source_dir.join(sqlcipher_src::LIBTOMCRYPT_INCLUDE_DIR);

    for path in [
        source_dir.join(sqlcipher_src::SOURCE_FILE),
        tomcrypt_include.join("tomcrypt.h"),
    ] {
        assert!(
            path.is_file(),
            "SQLCipher source not found: {} (check the sqlcipher-src version)",
            path.display()
        );
    }
    println!("cargo:rerun-if-changed={}", source_dir.display());

    // libtomcrypt's endianness detection knows MSVC's `_M_X64`/`_M_IX86` macros but not
    // `_M_ARM64`, and MSVC defines no `__BYTE_ORDER__`, so `tomcrypt_cfg.h` stops with "Cannot
    // detect endianness" on Windows ARM64. Every Windows target is little-endian; tell it the
    // word size too, because the header requires both.
    let msvc_arm64 = env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");

    // SQLCipher itself. The amalgamation only encrypts once `PRAGMA key` runs, so an unkeyed
    // connection keeps reading plain SQLite files.
    let mut sqlite = cc::Build::new();
    sqlite
        .file(source_dir.join(sqlcipher_src::SOURCE_FILE))
        .include(source_dir)
        .include(&tomcrypt_include)
        .flag_if_supported("-Wno-unused-parameter")
        .define("SQLITE_CORE", None)
        .define("SQLITE_DEFAULT_FOREIGN_KEYS", "1")
        .define("SQLITE_ENABLE_API_ARMOR", None)
        .define("SQLITE_ENABLE_COLUMN_METADATA", None)
        .define("SQLITE_ENABLE_DBSTAT_VTAB", None)
        .define("SQLITE_ENABLE_FTS3", None)
        .define("SQLITE_ENABLE_FTS3_PARENTHESIS", None)
        .define("SQLITE_ENABLE_FTS5", None)
        .define("SQLITE_ENABLE_JSON1", None)
        .define("SQLITE_ENABLE_LOAD_EXTENSION", "1")
        .define("SQLITE_ENABLE_MEMORY_MANAGEMENT", None)
        .define("SQLITE_ENABLE_RTREE", None)
        .define("SQLITE_ENABLE_STAT4", None)
        .define("SQLITE_SOUNDEX", None)
        .define("SQLITE_THREADSAFE", "1")
        .define("SQLITE_USE_URI", None)
        .define("HAVE_USLEEP", "1")
        .define("HAVE_ISNAN", None)
        .define("HAVE_STDINT_H", "1")
        // SQLCipher.
        .define("SQLITE_HAS_CODEC", None)
        // Keep temporary tables in memory: a temp file would otherwise be plaintext.
        .define("SQLITE_TEMP_STORE", "2")
        .define("SQLITE_EXTRA_INIT", "sqlcipher_extra_init")
        .define("SQLITE_EXTRA_SHUTDOWN", "sqlcipher_extra_shutdown")
        .define("SQLCIPHER_CRYPTO_LIBTOMCRYPT", None);
    if env::var_os("CARGO_FEATURE_UNLOCK_NOTIFY").is_some() {
        sqlite.define("SQLITE_ENABLE_UNLOCK_NOTIFY", None);
    }
    if !env::var_os("CARGO_CFG_WINDOWS").is_some() {
        sqlite.define("HAVE_LOCALTIME_R", None);
    }
    for define in LTC_DEFINES {
        sqlite.define(define, None);
    }
    // `ARGTYPE=4` makes a bad argument return CRYPT_INVALID_ARG instead of raising a signal.
    sqlite.define("ARGTYPE", "4");
    if msvc_arm64 {
        sqlite.define("ENDIAN_LITTLE", None);
        sqlite.define("ENDIAN_64BITWORD", None);
    }
    sqlite.warnings(false);
    sqlite.compile("sqlcipher");

    // libtomcrypt's AES/HMAC/SHA/PBKDF2/Fortuna provider, one translation unit per file. Each
    // needs `LTC_SOURCE` so the internal declarations are visible.
    let mut tomcrypt = cc::Build::new();
    tomcrypt
        .include(&tomcrypt_include)
        .files(
            sqlcipher_src::LIBTOMCRYPT_SOURCES
                .iter()
                .map(|source| source_dir.join(source)),
        )
        .define("LTC_SOURCE", None)
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-unused-variable");
    for define in LTC_DEFINES {
        tomcrypt.define(define, None);
    }
    tomcrypt.define("ARGTYPE", "4");
    if msvc_arm64 {
        tomcrypt.define("ENDIAN_LITTLE", None);
        tomcrypt.define("ENDIAN_64BITWORD", None);
    }
    tomcrypt.warnings(false);
    tomcrypt.compile("sqlcipher_libtomcrypt");

    // libtomcrypt reads the system CSPRNG through advapi32 on Windows and /dev/urandom
    // elsewhere, so only Windows needs an extra system library.
    if env::var_os("CARGO_CFG_WINDOWS").is_some() {
        println!("cargo:rustc-link-lib=advapi32");
    }

    // The bundled bindings are compiled in from `src/bindings.rs`; nothing to emit here.
}
