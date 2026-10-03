//! A small bb8 connection pool over tiberius. `tiberius::Client` is `Send` but not `Sync`
//! and drives one request at a time, so every operation checks out a pooled client.

use bb8::ManageConnection;
use tiberius::{Client, Config, EncryptionLevel};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

/// A tiberius client over a tokio TCP stream adapted to `futures::io`.
pub(crate) type SqlClient = Client<Compat<TcpStream>>;

/// Creates and validates SQL Server connections sharing one [`Config`].
#[derive(Clone, Debug)]
pub(crate) struct ConnectionManager {
    config: Config,
    /// SQL run on every new pooled connection (session initialization).
    init_sql: String,
}

impl ConnectionManager {
    pub(crate) fn new(config: Config, init_sql: String) -> Self {
        Self { config, init_sql }
    }
}

impl ManageConnection for ConnectionManager {
    type Connection = SqlClient;
    type Error = tiberius::error::Error;

    async fn connect(&self) -> Result<Self::Connection, Self::Error> {
        let tcp = TcpStream::connect(self.config.get_addr()).await?;
        tcp.set_nodelay(true)?;
        let mut client = Client::connect(self.config.clone(), tcp.compat_write()).await?;
        if !self.init_sql.trim().is_empty() {
            // Drain every result set so the connection is clean before it is pooled.
            client
                .simple_query(self.init_sql.clone())
                .await?
                .into_results()
                .await?;
        }
        Ok(client)
    }

    async fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.simple_query("SELECT 1").await?.into_results().await?;
        Ok(())
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

/// Map the shared TLS options onto tiberius's encryption level.
pub(crate) fn encryption_level(mode: rustgrid_core::TlsMode) -> EncryptionLevel {
    use rustgrid_core::TlsMode;
    match mode {
        // SQL Server always TLS-handshakes the login unless "not supported" is
        // advertised; encrypt the login only (then continue in the clear), which is
        // what ADO.NET's `Encrypt=false` does and works against default installs.
        TlsMode::Disabled => EncryptionLevel::Off,
        // Encrypt the login only, then continue in the clear.
        TlsMode::Preferred => EncryptionLevel::On,
        // Full encryption. Certificate trust is configured on the `Config`.
        TlsMode::Required | TlsMode::VerifyCa | TlsMode::VerifyIdentity => {
            EncryptionLevel::Required
        }
    }
}
