use crate::connection::ConnectOptions;
use crate::error::Error;
use crate::executor::Executor;
use crate::{MySqlConnectOptions, MySqlConnection};
use log::LevelFilter;
use sqlx_core::sql_str::AssertSqlSafe;
use sqlx_core::Url;
use std::time::Duration;

impl ConnectOptions for MySqlConnectOptions {
    type Connection = MySqlConnection;

    fn from_url(url: &Url) -> Result<Self, Error> {
        Self::parse_from_url(url)
    }

    fn to_url_lossy(&self) -> Url {
        self.build_url()
    }

    async fn connect(&self) -> Result<Self::Connection, Error>
    where
        Self::Connection: Sized,
    {
        let mut conn = MySqlConnection::establish(self).await?;

        // After the connection is established, we initialize by configuring a few
        // connection parameters

        // https://mariadb.com/kb/en/sql-mode/

        // PIPES_AS_CONCAT - Allows using the pipe character (ASCII 124) as string concatenation operator.
        //                   This means that "A" || "B" can be used in place of CONCAT("A", "B").

        // NO_ENGINE_SUBSTITUTION - If not set, if the available storage engine specified by a CREATE TABLE is
        //                          not available, a warning is given and the default storage
        //                          engine is used instead.

        // NO_ZERO_DATE - Don't allow '0000-00-00'. This is invalid in Rust.

        // NO_ZERO_IN_DATE - Don't allow 'YYYY-00-00'. This is invalid in Rust.

        // --

        // Setting the time zone allows us to assume that the output
        // from a TIMESTAMP field is UTC

        // --

        // https://mathiasbynens.be/notes/mysql-utf8mb4

        let mut sql_mode = Vec::new();
        if self.pipes_as_concat {
            sql_mode.push(r#"PIPES_AS_CONCAT"#);
        }
        if self.no_engine_substitution {
            sql_mode.push(r#"NO_ENGINE_SUBSTITUTION"#);
        }

        let mut options = Vec::new();
        if !sql_mode.is_empty() {
            options.push(format!(
                r#"sql_mode=(SELECT CONCAT(@@sql_mode, ',{}'))"#,
                sql_mode.join(",")
            ));
        }

        if let Some(timezone) = &self.timezone {
            options.push(format!(r#"time_zone='{}'"#, timezone));
        }

        if !options.is_empty() {
            // These session preferences are optional. A MySQL-compatible server may reject
            // them: OceanBase 3.x accepts neither a subquery in `SET` nor the
            // `NO_ENGINE_SUBSTITUTION` SQL mode and answers `1235`/`1064` (issue #62).
            // Connecting without the preferences is better than refusing the server, so a
            // server-side rejection is logged and skipped; transport errors still fail.
            let statement = format!(r#"SET {};"#, options.join(","));
            if let Err(error) = conn.execute(AssertSqlSafe(statement)).await {
                if matches!(&error, Error::Database(_)) {
                    log::warn!("the server rejected the optional login session settings: {error}");
                    conn.inner.session_settings_rejected = true;
                } else {
                    return Err(error);
                }
            }
        }

        // `SET NAMES` is sent as its own statement: some MySQL-compatible servers only accept
        // it as a whole statement, not as one option of a multi-assignment `SET`. OceanBase
        // 3.2.4 rejects the combined form with `1064 ... near 'utf8mb4;'` (issue #62).
        if self.set_names {
            // `utf8mb4` (the default) only exists from MySQL 5.5; older servers reject it
            // with "Unknown character set". Use their widest Unicode charset instead. An
            // explicit collation is left alone: it names a charset of its own.
            //
            // Only major.minor is compared: MariaDB 10+ announces itself as
            // "5.5.5-10.x.y-MariaDB", whose patch ("5-10") does not parse and reads as 0.
            // An unparseable version (major 0) is not assumed to be old.
            let (major, minor, _) = conn.inner.stream.server_version;
            let charset = if self.charset.eq_ignore_ascii_case("utf8mb4")
                && self.collation.is_none()
                && major != 0
                && (major, minor) < (5, 5)
            {
                log::warn!(
                    "server {:?} predates utf8mb4; using the utf8 character set",
                    conn.inner.stream.server_version
                );
                "utf8"
            } else {
                self.charset.as_str()
            };

            // As it turns out, we don't _have_ to set a collation if we don't want to.
            // We can let the server choose the default collation for the charset.
            let set_names = if let Some(collation) = &self.collation {
                format!(r#"NAMES {charset} COLLATE {collation}"#)
            } else {
                // Leaves the default collation up to the server,
                // but ensures statements and results are encoded using the proper charset.
                format!("NAMES {charset}")
            };

            conn.execute(AssertSqlSafe(format!("SET {set_names};")))
                .await?;
        }

        Ok(conn)
    }

    fn log_statements(mut self, level: LevelFilter) -> Self {
        self.log_settings.log_statements(level);
        self
    }

    fn log_slow_statements(mut self, level: LevelFilter, duration: Duration) -> Self {
        self.log_settings.log_slow_statements(level, duration);
        self
    }
}
