use std::time::Duration;

use async_trait::async_trait;
use rustgrid_core::{
    AdvancedFields, AutoIncrementRule, Connection, ConnectionConfig, ConnectionFieldChoice,
    ConnectionFieldKind, ConnectionFieldLabel, ConnectionFieldSpec, ConnectionFormSpec,
    ConnectionHomePage, ConnectionPage, ConnectionStandardField, DatabaseEditorSpec,
    DefaultSuggestion, DesignerColumnOption, DesignerFieldColumn, DesignerIndexColumn,
    DesignerIndexKind, DesignerTab, Driver, DriverCapabilities, DriverDescriptor, DriverDialect,
    DriverIconStyle, DriverId, Error, Result, TableDesignerSpec,
};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::connection::SqliteConnection;

/// The engine option selecting the database file's encryption: `none` (plain SQLite),
/// `sqlcipher4` (SQLCipher 4's default settings) or `sqlcipher3` (SQLCipher 3 compatibility, for
/// databases created by older SQLCipher releases).
const OPTION_ENCRYPTION: &str = "sqlite.encryption";
const ENCRYPTION_NONE: &str = "none";
const ENCRYPTION_SQLCIPHER4: &str = "sqlcipher4";
const ENCRYPTION_SQLCIPHER3: &str = "sqlcipher3";

/// The SQLite driver. SQLite is a file-based engine: the database is a single file and there is no
/// server or user account. The profile's `database` field holds the file path; for a SQLCipher
/// database the connection password carries its key.
#[derive(Debug, Default)]
pub struct SqliteDriver;

impl SqliteDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Driver for SqliteDriver {
    fn id(&self) -> DriverId {
        DriverId::new("sqlite")
    }

    fn display_name(&self) -> String {
        "SQLite".to_string()
    }

    fn default_port(&self) -> u16 {
        0
    }

    fn is_file_based(&self) -> bool {
        true
    }

    fn supports_database_management(&self) -> bool {
        false
    }

    fn supports_users(&self) -> bool {
        false
    }

    fn supports_routines(&self) -> bool {
        false
    }

    fn database_editor(&self) -> DatabaseEditorSpec {
        DatabaseEditorSpec::default()
    }

    fn descriptor(&self) -> DriverDescriptor {
        DriverDescriptor {
            // SQLite keeps no table options and no table comment, and its designer DDL emits no
            // per-column collation, so those tabs and controls are left off rather than shown dead.
            table_designer: TableDesignerSpec {
                tabs: vec![
                    DesignerTab::Fields,
                    DesignerTab::Indexes,
                    DesignerTab::ForeignKeys,
                    DesignerTab::Sql,
                ],
                column_options: vec![
                    DesignerColumnOption::Default,
                    DesignerColumnOption::AutoIncrement,
                ],
                field_columns: vec![
                    DesignerFieldColumn::Length,
                    DesignerFieldColumn::NotNull,
                    DesignerFieldColumn::Generated,
                    DesignerFieldColumn::Key,
                ],
                option_fields: Vec::new(),
                default_suggestions: vec![
                    DefaultSuggestion::for_types(
                        "CURRENT_TIMESTAMP",
                        &["date", "datetime", "timestamp", "text"],
                    ),
                    DefaultSuggestion::for_types("CURRENT_DATE", &["date", "text"]),
                    DefaultSuggestion::for_types("CURRENT_TIME", &["time", "text"]),
                    DefaultSuggestion::for_types(
                        "0",
                        &[
                            "integer", "int", "smallint", "bigint", "real", "double", "float",
                            "numeric", "decimal",
                        ],
                    ),
                    DefaultSuggestion::for_types("''", &["text", "varchar", "char", "clob"]),
                    DefaultSuggestion::for_types("true", &["boolean", "bool", "integer", "int"]),
                    DefaultSuggestion::for_types("false", &["boolean", "bool", "integer", "int"]),
                    DefaultSuggestion::every("NULL"),
                ],
                // `AUTOINCREMENT` only exists on an `INTEGER PRIMARY KEY`.
                auto_increment: AutoIncrementRule {
                    types: vec!["integer"],
                    ..AutoIncrementRule::default()
                },
                // SQLite indexes sort per column; there is no prefix length.
                index_columns: vec![DesignerIndexColumn::Sort],
                // `CREATE [UNIQUE] INDEX` is all SQLite has: no type of its own and no comment.
                index_kinds: vec![
                    DesignerIndexKind {
                        label: "NORMAL",
                        unique: false,
                        types: None,
                    },
                    DesignerIndexKind {
                        label: "UNIQUE",
                        unique: true,
                        types: None,
                    },
                ],
                index_comment: false,
                trigger_definition: Default::default(),
            },
            id: DriverId::new("sqlite"),
            display_name: "SQLite".to_string(),
            default_port: 0,
            is_file_based: true,
            icon: "icons/sqlite.svg",
            // A slightly brighter blue than the logo's #003b57 so it stays legible on dark themes.
            icon_style: DriverIconStyle::Brand(0x0f80cc),
            // SQLite has no databases, server accounts, routines or schemas of its own.
            capabilities: DriverCapabilities::none(),
            database_editor: DatabaseEditorSpec::default(),
            // A file-based engine: the 常规 page edits one database file, and there is no TLS or
            // tunnel (a local file has no network endpoint), only 高级.
            connection_form: ConnectionFormSpec {
                home: ConnectionHomePage::File,
                tabs: vec![ConnectionPage::Advanced],
                optional_fields: Vec::new(),
                // SQLite's "connect timeout" is really the lock / busy timeout: relabel it so it
                // does not read as a TCP handshake. The standard password field is the SQLCipher
                // key, shown only while an encrypted database is selected; it goes through the
                // encrypted secret store like any server password.
                labels: vec![
                    ConnectionFieldLabel {
                        field: ConnectionStandardField::ConnectTimeout,
                        label_key: "form.advanced.busy_timeout",
                        hint_key: Some("form.advanced.busy_timeout_hint"),
                        visible_when: None,
                    },
                    ConnectionFieldLabel {
                        field: ConnectionStandardField::Password,
                        label_key: "form.sqlite.encryption_key",
                        hint_key: Some("form.sqlite.encryption_key_hint"),
                        visible_when: Some((
                            OPTION_ENCRYPTION,
                            &[ENCRYPTION_SQLCIPHER4, ENCRYPTION_SQLCIPHER3],
                        )),
                    },
                ],
                options: vec![ConnectionFieldSpec {
                    key: OPTION_ENCRYPTION,
                    label_key: "form.sqlite.encryption",
                    hint_key: None,
                    kind: ConnectionFieldKind::Select(vec![
                        ConnectionFieldChoice {
                            value: ENCRYPTION_NONE,
                            label_key: "form.sqlite.encryption_none",
                        },
                        ConnectionFieldChoice {
                            value: ENCRYPTION_SQLCIPHER4,
                            label_key: "form.sqlite.encryption_sqlcipher4",
                        },
                        ConnectionFieldChoice {
                            value: ENCRYPTION_SQLCIPHER3,
                            label_key: "form.sqlite.encryption_sqlcipher3",
                        },
                    ]),
                    page: ConnectionPage::General,
                    visible_when: None,
                }],
                // SQLite reads the connect timeout (as a busy timeout), the read-only flag and the
                // init SQL; it has no query timeout or keep-alive.
                advanced: AdvancedFields {
                    connect_timeout: true,
                    read_only: true,
                    init_sql: true,
                    ..AdvancedFields::NONE
                },
            },
            order: 40,
        }
    }

    fn dialect(&self) -> DriverDialect {
        DriverDialect::Sqlite
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Connection>> {
        // The file path is carried in `database`; fall back to `host` so an older/hand-made profile
        // (or a copied MySQL one) still resolves.
        let path = config
            .database
            .clone()
            .filter(|path| !path.trim().is_empty())
            .or_else(|| (!config.host.trim().is_empty()).then(|| config.host.trim().to_string()))
            .ok_or_else(|| Error::Connection("a SQLite database file is required".to_string()))?;

        let mut options = SqliteConnectOptions::new()
            .filename(&path)
            // Only open an existing file. Creating a new database is a separate, explicit flow so a
            // typo cannot silently produce an empty file.
            .create_if_missing(false)
            .foreign_keys(true)
            .read_only(config.settings.read_only);
        if let Some(seconds) = config
            .settings
            .connect_timeout
            .filter(|seconds| *seconds > 0)
        {
            // SQLite has no connection timeout; the busy timeout is the closest equivalent (how
            // long it waits on a locked database before failing).
            options = options.busy_timeout(Duration::from_secs(seconds));
        }

        // SQLCipher's key is the connection password (kept in the encrypted secret store, never in
        // the profile). An encrypted database cannot be opened without one, so a missing key is an
        // authentication failure: the app then prompts for it and retries.
        let encryption = match config
            .options
            .get(OPTION_ENCRYPTION)
            .map(String::as_str)
            .unwrap_or(ENCRYPTION_NONE)
        {
            ENCRYPTION_SQLCIPHER3 => Some(SqliteCipher::new(3, config.password.as_deref())),
            ENCRYPTION_SQLCIPHER4 => Some(SqliteCipher::new(4, config.password.as_deref())),
            _ => None,
        };
        let encrypted = encryption.is_some();
        if let Some(cipher) = encryption.as_ref()
            && cipher.password.is_empty()
        {
            return Err(Error::Authentication(
                "the SQLCipher database requires a password".to_string(),
            ));
        }
        // A database that is not encrypted ignores a saved password; do not send it a key.
        let encryption = encryption.filter(|cipher| !cipher.password.is_empty());

        // Check the key once before the pool exists. A pool retries a failing `after_connect` until
        // its acquire timeout, which would turn a wrong password into an unhelpful "pool timed out"
        // instead of the authentication failure the password prompt is keyed on.
        if let Some(cipher) = encryption.as_ref() {
            use sqlx::{ConnectOptions as _, Connection as _};
            let mut probe = options
                .clone()
                .connect()
                .await
                .map_err(|error| map_connect_error(error, encrypted))?;
            cipher
                .verify(&mut probe)
                .await
                .map_err(|error| map_connect_error(error, encrypted))?;
            let _ = probe.close().await;
        }

        let init_sql = config.settings.init_sql.trim().to_string();
        let pool_options = SqlitePoolOptions::new().max_connections(5);
        let pool_options = if encryption.is_none() && init_sql.is_empty() {
            pool_options
        } else {
            pool_options.after_connect(move |connection, _metadata| {
                let encryption = encryption.clone();
                let init_sql = init_sql.clone();
                Box::pin(async move {
                    if let Some(cipher) = encryption {
                        cipher.apply(connection).await?;
                    }
                    if !init_sql.is_empty() {
                        sqlx::query(sqlx::AssertSqlSafe(init_sql))
                            .execute(connection)
                            .await?;
                    }
                    Ok(())
                })
            })
        };

        let pool = pool_options
            .connect_with(options)
            .await
            .map_err(|error| map_connect_error(error, encrypted))?;

        Ok(Box::new(SqliteConnection::new(pool)))
    }
}

/// The SQLCipher settings applied to every pooled connection. SQLCipher requires the key before the
/// database is first read, and the compatibility level immediately after it (also before that
/// first read), so this cannot go through `SqliteConnectOptions::pragma` (which has no way to put
/// `cipher_compatibility` after `key`).
#[derive(Clone)]
struct SqliteCipher {
    /// SQLCipher compatibility level (3 for databases created by SQLCipher 3, 4 otherwise).
    compatibility: u8,
    password: String,
}

impl SqliteCipher {
    fn new(compatibility: u8, password: Option<&str>) -> Self {
        Self {
            compatibility,
            password: password.unwrap_or_default().to_string(),
        }
    }

    async fn apply(&self, connection: &mut sqlx::SqliteConnection) -> sqlx::Result<()> {
        // `PRAGMA` takes no bind parameters, so the passphrase is rendered as a SQL string literal.
        // Doubling the quote is SQLite's escape for a literal quote and keeps a key containing
        // quotes or semicolons from breaking out of the statement.
        let mut sql = format!("PRAGMA key = '{}';", self.password.replace('\'', "''"));
        // The compatibility level follows the key: `cipher_compatibility` adjusts the current
        // connection's defaults (it must run before the first page is read), and SQLCipher 4
        // cannot open a v3 database without it.
        if self.compatibility != 4 {
            sql.push_str(&format!(
                " PRAGMA cipher_compatibility = {};",
                self.compatibility
            ));
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
            .execute(connection)
            .await?;
        Ok(())
    }

    /// Apply the key and read the schema, which is what actually verifies it: `PRAGMA key` itself
    /// succeeds for any value and the first page read is where a wrong password fails.
    async fn verify(&self, connection: &mut sqlx::SqliteConnection) -> sqlx::Result<()> {
        self.apply(connection).await?;
        sqlx::query("SELECT count(*) FROM sqlite_master")
            .fetch_one(connection)
            .await?;
        Ok(())
    }
}

fn map_connect_error(error: sqlx::Error, encrypted: bool) -> Error {
    if is_cipher_error(&error) {
        if encrypted {
            // A wrong SQLCipher key reports "file is not a database" (SQLITE_NOTADB), which the
            // app must treat as an authentication failure so it can prompt for the password.
            return Error::Authentication(format!("SQLCipher: {error}"));
        }
        // The plain-SQLite mode also sees this error for an encrypted file: the user selected the
        // wrong encryption mode rather than a corrupt database.
        return Error::Connection(
            "the file is not a plain SQLite database; if it is SQLCipher-encrypted, choose an \
             encryption mode and enter its password"
                .to_string(),
        );
    }
    Error::Connection(error.to_string())
}

fn is_cipher_error(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(error) = error else {
        return false;
    };
    if error.code().as_deref() == Some("26") {
        return true;
    }
    let message = error.message().to_ascii_lowercase();
    message.contains("not a database") || message.contains("file is encrypted")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQLite relabels the 高级 connect timeout as its lock / busy timeout and honors it (plus the
    /// read-only flag and the init SQL); it must not offer the fields it cannot apply.
    #[test]
    fn connection_form_relabels_the_busy_timeout_and_gates_the_advanced_fields() {
        let descriptor = SqliteDriver::new().descriptor();
        let form = &descriptor.connection_form;

        let label = form
            .labels
            .iter()
            .find(|label| label.field == ConnectionStandardField::ConnectTimeout)
            .expect("the connect timeout should be relabelled");
        assert_eq!(label.label_key, "form.advanced.busy_timeout");
        assert!(label.hint_key.is_some());

        assert!(form.advanced.connect_timeout);
        assert!(form.advanced.read_only);
        assert!(form.advanced.init_sql);
        assert!(!form.advanced.query_timeout);
        assert!(!form.advanced.keepalive);
    }

    /// SQLCipher is an encryption *mode* on the file page, and its key is the standard password
    /// field (so it goes through the encrypted secret store), shown only while a SQLCipher mode is
    /// selected.
    #[test]
    fn connection_form_offers_sqlcipher_and_shows_the_standard_password_as_its_key() {
        let descriptor = SqliteDriver::new().descriptor();
        let form = &descriptor.connection_form;

        let encryption = form
            .options
            .iter()
            .find(|option| option.key == OPTION_ENCRYPTION)
            .expect("the encryption control should be declared");
        assert_eq!(encryption.page, ConnectionPage::General);
        let ConnectionFieldKind::Select(choices) = &encryption.kind else {
            panic!("the encryption control should be a dropdown");
        };
        assert_eq!(choices[0].value, ENCRYPTION_NONE);
        assert!(
            choices
                .iter()
                .any(|choice| choice.value == ENCRYPTION_SQLCIPHER4)
        );
        assert!(
            choices
                .iter()
                .any(|choice| choice.value == ENCRYPTION_SQLCIPHER3)
        );

        let key = form
            .labels
            .iter()
            .find(|label| label.field == ConnectionStandardField::Password)
            .expect("the password field should be relabelled as the SQLCipher key");
        assert_eq!(key.label_key, "form.sqlite.encryption_key");
        assert_eq!(
            key.visible_when,
            Some((
                OPTION_ENCRYPTION,
                &[ENCRYPTION_SQLCIPHER4, ENCRYPTION_SQLCIPHER3] as &[&str]
            ))
        );
    }
}
