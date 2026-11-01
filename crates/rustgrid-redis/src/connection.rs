//! The [`rustgrid_core::Connection`] implementation for Redis.
//!
//! Keys are the engine's objects, so the driver maps a database to a container and a key to a
//! "table" whose columns depend on the key's type. All reads go through the shared command helpers
//! below, which also keep the per-database connections (`SELECT` is connection state, and the
//! multiplexed connection is shared by concurrent tabs, so each database gets its own connection).

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::stream::{self, Stream, StreamExt};
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::sentinel::{SentinelClientBuilder, SentinelServerType};
use redis::{
    Client, Cmd, ConnectionAddr, ConnectionInfo, ErrorKind, Msg, ProtocolVersion,
    RedisConnectionInfo, RedisError, TlsCertificates, Value,
};
use rustgrid_core::{
    BackupObjectKind, CellValue, ColumnInfo, Connection, ConnectionConfig, DatabaseInfo,
    DatabaseOptions, DriverId, Error, ObjectCreate, ObjectDump, ObjectKind, ObjectNamespace,
    PageRequest, QueryResult, Result, RowInsert, RowUpdate, TableInfo, TablePage, TableSchema,
    TableStatus, Watch, WatchEvent, WatchSpec,
};
use rustgrid_tunnel::Tunnel;

use crate::command;
use crate::driver::{
    DEFAULT_KEY_LIMIT, OPT_CLUSTER_NODES, OPT_KEY_LIMIT, OPT_MODE, OPT_SENTINEL_HOSTS,
    OPT_SENTINEL_MASTER, OPT_SENTINEL_PASSWORD,
};
use crate::keys::KeyType;
use crate::reply;
use crate::version::ServerInfo;

/// The largest collection this driver will materialise for a single key. `HGETALL`/`SMEMBERS`
/// have no offset, so a page is cut from the full value; the cap keeps a multi-million-field hash
/// from exhausting the UI process, and the error tells the user which console command to use
/// instead.
const COLLECTION_FETCH_LIMIT: u64 = 200_000;

/// How many keys are probed in parallel when the object list asks for every key's status.
const STATUS_CONCURRENCY: usize = 16;

/// The commands per `SCAN` round.
const SCAN_COUNT: usize = 1000;

/// How many leading bytes of a string key the object list's 值 preview reads (`GETRANGE`, so a
/// multi-megabyte value is never transferred just to show a one-line preview).
const PREVIEW_BYTES: usize = 200;

/// How the client reaches the server.
enum Mode {
    Standalone,
    Sentinel(Box<SentinelSetup>),
    Cluster,
}

/// TLS parameters resolved from the connection settings.
#[derive(Clone, Default)]
struct TlsPlan {
    enabled: bool,
    /// Skip certificate verification (`Required`; Redis' `Preferred` degrades to this).
    insecure: bool,
    /// `Preferred` tries TLS and falls back to a plaintext connection when the handshake fails.
    plain_fallback: bool,
    certs: Option<TlsCertificates>,
}

/// A standalone server's fixed address and credentials, from which one client per database is
/// built (`SELECT` is part of the connection, not a command this driver sends).
#[derive(Clone)]
struct SingleSetup {
    host: String,
    port: u16,
    tls: TlsPlan,
    username: Option<String>,
    password: Option<String>,
    connect_timeout: Option<Duration>,
}

struct SentinelSetup {
    nodes: Vec<ConnectionAddr>,
    master: String,
    sentinel_password: Option<String>,
    tls: TlsPlan,
    username: Option<String>,
    password: Option<String>,
    connect_timeout: Option<Duration>,
}

impl SentinelSetup {
    async fn connect(&self, db: i64, timeout: Option<Duration>) -> Result<MultiplexedConnection> {
        let mut builder = SentinelClientBuilder::new(
            self.nodes.clone(),
            self.master.clone(),
            SentinelServerType::Master,
        )
        .map_err(connect_error)?;
        builder = builder.set_client_to_redis_db(db);
        if let Some(username) = &self.username {
            builder = builder.set_client_to_redis_username(username.clone());
        }
        if let Some(password) = &self.password {
            builder = builder.set_client_to_redis_password(password.clone());
        }
        if self.tls.enabled {
            builder = builder.set_client_to_redis_tls_mode(if self.tls.insecure {
                redis::TlsMode::Insecure
            } else {
                redis::TlsMode::Secure
            });
            builder = builder.set_client_to_sentinel_tls_mode(if self.tls.insecure {
                redis::TlsMode::Insecure
            } else {
                redis::TlsMode::Secure
            });
        }
        if let Some(certs) = &self.tls.certs {
            builder = builder.set_client_to_redis_certificates(certs.clone());
            builder = builder.set_client_to_sentinel_certificates(certs.clone());
        }
        if let Some(password) = &self.sentinel_password {
            builder = builder.set_client_to_sentinel_password(password.clone());
        }
        let mut client = builder.build().map_err(connect_error)?;

        let mut config = redis::AsyncConnectionConfig::new();
        if let Some(timeout) = self.connect_timeout {
            config = config.set_connection_timeout(timeout);
        }
        if let Some(timeout) = timeout {
            config = config.set_response_timeout(timeout);
        }
        client
            .get_async_connection_with_config(&config)
            .await
            .map_err(connect_error)
    }

    /// A plain client for the master this setup discovered, used by the live streams (`MONITOR`
    /// and pub/sub need a `Client`, not the sentinel-wrapped connection).
    fn client_for(&self, host: &str, port: u16, db: i64) -> Result<Client> {
        SingleSetup {
            host: host.to_string(),
            port,
            tls: self.tls.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            connect_timeout: self.connect_timeout,
        }
        .client(db)
    }
}

impl SingleSetup {
    fn client(&self, db: i64) -> Result<Client> {
        self.client_for(&self.host, self.port, db)
    }

    fn client_for(&self, host: &str, port: u16, db: i64) -> Result<Client> {
        let address = if self.tls.enabled {
            ConnectionAddr::TcpTls {
                host: host.to_string(),
                port,
                insecure: self.tls.insecure,
                tls_params: None,
            }
        } else {
            ConnectionAddr::Tcp(host.to_string(), port)
        };
        let info = ConnectionInfo {
            addr: address,
            redis: RedisConnectionInfo {
                db,
                username: self.username.clone(),
                password: self.password.clone(),
                protocol: ProtocolVersion::RESP2,
            },
        };
        match &self.tls.certs {
            Some(certs) => Client::build_with_tls(info, certs.clone()).map_err(connect_error),
            None => Client::open(info).map_err(connect_error),
        }
    }
}

/// One Redis connection: a standalone server, a sentinel-managed master, or a cluster.
pub struct RedisConnection {
    mode: Mode,
    default_db: i64,
    key_limit: usize,
    read_only: bool,
    response_timeout: Option<Duration>,
    single: Option<SingleSetup>,
    cluster: Option<ClusterConnection>,
    /// A template for reaching individual cluster nodes when scanning their shards
    /// (`host`/`port` are placeholders; the credentials and TLS plan are what matter).
    node_setup: Option<SingleSetup>,
    /// Connections per database for standalone/sentinel (cluster has a single keyspace).
    connections: tokio::sync::Mutex<HashMap<i64, MultiplexedConnection>>,
    info: ServerInfo,
    /// The tunnel forwarding this connection, kept alive for as long as the connection is.
    _tunnel: Option<Tunnel>,
}

impl std::fmt::Debug for RedisConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RedisConnection")
            .field(
                "mode",
                &match self.mode {
                    Mode::Standalone => "standalone",
                    Mode::Sentinel(_) => "sentinel",
                    Mode::Cluster => "cluster",
                },
            )
            .field("version", &self.info.version_text)
            .finish()
    }
}

impl RedisConnection {
    /// Builds the connection from a saved profile: resolves the mode, opens the seed connection and
    /// reads the server version that gates the version-specific commands.
    pub async fn open(config: &ConnectionConfig) -> Result<Self> {
        let mode = config
            .options
            .get(OPT_MODE)
            .map(String::as_str)
            .unwrap_or("standalone");
        let default_db = parse_db(config.database.as_deref())?;
        let key_limit = parse_key_limit(config.options.get(OPT_KEY_LIMIT))?;
        let tls = TlsPlan::resolve(&config.settings)?;
        let mut username = text(config.username.as_str());
        // `default` is Redis's built-in user. Sending `AUTH default <password>` needs Redis 6+;
        // `AUTH <password>` reaches the same default user (and is the only form Redis 5 knows),
        // so a `default` user name is normalised away for compatibility.
        if username
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case("default"))
        {
            username = None;
        }
        let password = config.password.clone().filter(|value| !value.is_empty());
        // An ACL username without a password cannot authenticate (Redis 6 `AUTH user ""` is a
        // different thing); a blank password field means "no AUTH at all".
        if password.is_none() {
            username = None;
        }
        let connect_timeout = config
            .settings
            .connect_timeout
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs);
        let response_timeout = config
            .settings
            .query_timeout
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs);

        let host = config.host.trim().to_string();
        let port = config.port;
        if host.is_empty() {
            return Err(Error::Config("the host is required".to_string()));
        }

        // A tunnel forwards a single target, so it only makes sense for a standalone server; the
        // addresses a sentinel or a cluster returns are not reachable through that forwarder.
        let (host, port, tunnel) = if config.settings.tunnel.is_empty() {
            (host, port, None)
        } else if mode == "standalone" {
            if tls.enabled && !tls.insecure {
                return Err(Error::Config(
                    "TLS certificate verification cannot run through a tunnel: the certificate \
                     is issued for the server, not for 127.0.0.1. Disable verification or connect \
                     directly."
                        .to_string(),
                ));
            }
            let tunnel = Tunnel::start(config.settings.tunnel.clone(), host.clone(), port)
                .await
                .map_err(Error::Connection)?;
            ("127.0.0.1".to_string(), tunnel.local_port(), Some(tunnel))
        } else {
            return Err(Error::Config(
                "a tunnel is only supported for a standalone Redis server".to_string(),
            ));
        };

        let mut connection = Self {
            mode: Mode::Standalone,
            default_db,
            key_limit,
            read_only: config.settings.read_only,
            response_timeout,
            single: None,
            cluster: None,
            node_setup: None,
            connections: tokio::sync::Mutex::new(HashMap::new()),
            info: ServerInfo {
                version: crate::version::ServerVersion::UNKNOWN,
                version_text: String::new(),
                flavor: "redis".to_string(),
            },
            _tunnel: tunnel,
        };

        match mode {
            "cluster" => {
                if default_db != 0 {
                    return Err(Error::Config(
                        "Redis Cluster has a single keyspace; the database must be 0".to_string(),
                    ));
                }
                let seeds = parse_nodes(config.options.get(OPT_CLUSTER_NODES), port, port)?;
                let seeds = if seeds.is_empty() {
                    vec![(host.clone(), port)]
                } else {
                    seeds
                };
                let mut builder = ClusterClient::builder(
                    seeds
                        .iter()
                        .map(|(host, port)| {
                            connection_info(host, *port, 0, &tls, &username, &password)
                        })
                        .collect::<Vec<_>>(),
                );
                if let Some(username) = &username {
                    builder = builder.username(username.clone());
                }
                if let Some(password) = &password {
                    builder = builder.password(password.clone());
                }
                if tls.enabled {
                    if tls.insecure {
                        // `certs()` would force full verification; "Required" means encrypt only.
                        builder = builder.tls(redis::TlsMode::Insecure);
                    } else if let Some(certs) = &tls.certs {
                        builder = builder.certs(certs.clone());
                    } else {
                        builder = builder.tls(redis::TlsMode::Secure);
                    }
                }
                let client = builder.build().map_err(connect_error)?;
                let mut cluster_config = redis::cluster::ClusterConfig::new();
                if let Some(timeout) = connect_timeout {
                    cluster_config = cluster_config.set_connection_timeout(timeout);
                }
                if let Some(timeout) = response_timeout {
                    cluster_config = cluster_config.set_response_timeout(timeout);
                }
                let cluster = client
                    .get_async_connection_with_config(cluster_config)
                    .await
                    .map_err(connect_error)?;
                connection.mode = Mode::Cluster;
                connection.info = read_server_info_cluster(&cluster).await;
                connection.node_setup = Some(SingleSetup {
                    host: String::new(),
                    port: 0,
                    tls: tls.clone(),
                    username: username.clone(),
                    password: password.clone(),
                    connect_timeout,
                });
                connection.cluster = Some(cluster);
            }
            "sentinel" => {
                let master = config
                    .options
                    .get(OPT_SENTINEL_MASTER)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| {
                        Error::Config("the sentinel master name is required".to_string())
                    })?;
                let hosts = parse_nodes(config.options.get(OPT_SENTINEL_HOSTS), 26379, 26379)?;
                if hosts.is_empty() {
                    return Err(Error::Config(
                        "at least one sentinel address is required".to_string(),
                    ));
                }
                let nodes = hosts
                    .iter()
                    .map(|(host, port)| {
                        if tls.enabled {
                            ConnectionAddr::TcpTls {
                                host: host.clone(),
                                port: *port,
                                insecure: tls.insecure,
                                tls_params: None,
                            }
                        } else {
                            ConnectionAddr::Tcp(host.clone(), *port)
                        }
                    })
                    .collect::<Vec<_>>();
                let sentinel_password = config
                    .options
                    .get(OPT_SENTINEL_PASSWORD)
                    .filter(|value| !value.is_empty())
                    .cloned();
                let setup = SentinelSetup {
                    nodes,
                    master,
                    sentinel_password,
                    tls: tls.clone(),
                    username,
                    password,
                    connect_timeout,
                };
                let mut conn = setup.connect(default_db, response_timeout).await?;
                connection.info = read_server_info(&mut conn).await;
                connection.connections.lock().await.insert(default_db, conn);
                connection.mode = Mode::Sentinel(Box::new(setup));
            }
            _ => {
                let setup = SingleSetup {
                    host,
                    port,
                    tls,
                    username,
                    password,
                    connect_timeout,
                };
                let mut conn = match open_single(&setup, default_db, response_timeout).await {
                    Ok(connection) => connection,
                    // `Preferred` means "encrypt when the server speaks TLS"; a server on the
                    // plaintext port answers the handshake with garbage, so retry without it.
                    Err(error) if setup.tls.plain_fallback => {
                        let mut plain = setup.clone();
                        plain.tls = TlsPlan::default();
                        open_single(&plain, default_db, response_timeout)
                            .await
                            .map_err(|_| error)?
                    }
                    Err(error) => return Err(error),
                };
                connection.info = read_server_info(&mut conn).await;
                connection.mode = Mode::Standalone;
                connection.single = Some(setup);
                connection.connections.lock().await.insert(default_db, conn);
            }
        }

        Ok(connection)
    }

    fn is_cluster(&self) -> bool {
        matches!(self.mode, Mode::Cluster)
    }

    fn db_for(&self, database: Option<&str>) -> Result<i64> {
        match database {
            Some(name) if !name.trim().is_empty() => parse_db(Some(name)),
            _ => Ok(self.default_db),
        }
    }

    /// The connection for one database (standalone/sentinel mode). Each database gets its own
    /// connection because the selected database is connection state, and tabs share the connection.
    async fn database_connection(&self, db: i64) -> Result<MultiplexedConnection> {
        let Some(setup) = &self.single else {
            return Err(Error::Query(
                "this connection is not a standalone server".to_string(),
            ));
        };
        if let Some(connection) = self.connections.lock().await.get(&db) {
            return Ok(connection.clone());
        }
        let client = setup.client(db)?;
        let connection =
            connect_client(&client, setup.connect_timeout, self.response_timeout).await?;
        self.connections.lock().await.insert(db, connection.clone());
        Ok(connection)
    }

    async fn handle(&self, db: i64) -> Result<Handle> {
        if let Some(cluster) = &self.cluster {
            return Ok(Handle::Cluster(cluster.clone()));
        }
        match &self.mode {
            Mode::Sentinel(setup) => {
                if let Some(connection) = self.connections.lock().await.get(&db) {
                    return Ok(Handle::Single(connection.clone()));
                }
                let connection = setup.connect(db, self.response_timeout).await?;
                self.connections.lock().await.insert(db, connection.clone());
                Ok(Handle::Single(connection))
            }
            _ => Ok(Handle::Single(self.database_connection(db).await?)),
        }
    }

    /// Runs one command against `db`.
    async fn run(&self, db: i64, cmd: &Cmd) -> Result<Value> {
        let mut handle = self.handle(db).await?;
        match &mut handle {
            Handle::Single(connection) => connection
                .req_packed_command(cmd)
                .await
                .map_err(query_error),
            Handle::Cluster(connection) => connection
                .req_packed_command(cmd)
                .await
                .map_err(query_error),
        }
    }

    /// Runs one command and reads its reply as text.
    async fn run_text(&self, db: i64, cmd: &Cmd) -> Result<String> {
        let value = self.run(db, cmd).await?;
        reply::reply_text(&value).ok_or_else(|| {
            Error::Query(format!("unexpected reply: {}", reply::nested_text(&value)))
        })
    }

    async fn key_type(&self, db: i64, key: &str) -> Result<KeyType> {
        let raw = self.run_text(db, &cmd_value("TYPE", &[key])).await?;
        if raw.trim() == "none" {
            return Err(Error::Query("the key does not exist".to_string()));
        }
        Ok(KeyType::parse(&raw))
    }

    /// Lists keys with `SCAN`, stopping at `limit` keys.
    ///
    /// A cluster shards its keyspace across masters and `SCAN` only sees the node it runs on, so
    /// cluster mode scans every master from `CLUSTER SLOTS` and merges the result.
    async fn scan_keys(&self, db: i64, limit: usize, pattern: Option<&str>) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        if self.is_cluster() {
            let masters = self.cluster_masters().await?;
            let scans = masters.into_iter().map(|(host, port)| async move {
                self.scan_one_node(&host, port, limit, pattern)
                    .await
                    .unwrap_or_default()
            });
            let results = stream::iter(scans)
                .buffer_unordered(8)
                .collect::<Vec<_>>()
                .await;
            for node_keys in results {
                keys.extend(node_keys);
            }
        } else {
            let handle = self.handle(db).await?;
            let mut connection = match handle {
                Handle::Single(connection) => connection,
                Handle::Cluster(_) => unreachable!("cluster handled above"),
            };
            keys = scan_connection(&mut connection, limit, pattern).await?;
        }
        keys.sort();
        keys.dedup();
        keys.truncate(limit);
        Ok(keys)
    }

    /// Scans one node directly (cluster mode), reusing the connection's credentials and TLS plan.
    async fn scan_one_node(
        &self,
        host: &str,
        port: u16,
        limit: usize,
        pattern: Option<&str>,
    ) -> Result<Vec<String>> {
        let Some(setup) = &self.node_setup else {
            return Err(Error::Query(
                "this connection cannot reach individual cluster nodes".to_string(),
            ));
        };
        let client = setup.client_for(host, port, 0)?;
        let mut connection =
            connect_client(&client, setup.connect_timeout, self.response_timeout).await?;
        scan_connection(&mut connection, limit, pattern).await
    }

    /// The cluster masters as `(host, port)` (one per slot owner).
    async fn cluster_masters(&self) -> Result<Vec<(String, u16)>> {
        let Some(cluster) = &self.cluster else {
            return Ok(Vec::new());
        };
        let mut cluster = cluster.clone();
        let value = cluster
            .req_packed_command(&cmd_value("CLUSTER", &["SLOTS"]))
            .await
            .map_err(query_error)?;
        let mut masters = Vec::new();
        if let Value::Array(slots) = value {
            for slot in slots {
                let Value::Array(parts) = slot else { continue };
                // [start, end, [ip, port, id, ...], ...]
                if let Some(Value::Array(node)) = parts.get(2)
                    && let (Some(host), Some(port)) = (
                        node.first().and_then(reply::reply_text),
                        node.get(1).and_then(reply::reply_integer),
                    )
                {
                    let entry = (host, port as u16);
                    if !masters.contains(&entry) {
                        masters.push(entry);
                    }
                }
            }
        }
        Ok(masters)
    }
}

impl RedisConnection {
    async fn status_for(&self, db: i64, key: &str) -> Result<TableStatus> {
        let raw = self.run_text(db, &cmd_value("TYPE", &[key])).await?;
        let key_type = KeyType::parse(&raw);
        if !key_type.is_supported() {
            return Ok(TableStatus {
                engine: Some(key_type.label().to_string()),
                ..Default::default()
            });
        }
        let ttl = self
            .run(db, &cmd_value("TTL", &[key]))
            .await
            .ok()
            .and_then(|value| reply::reply_integer(&value))
            .filter(|ttl| *ttl != -2);
        let memory = if self.info.version.has_memory_usage() {
            self.run(db, &cmd_value("MEMORY", &["USAGE", key]))
                .await
                .ok()
                .and_then(|value| reply::reply_integer(&value))
                .map(|bytes| bytes as u64)
        } else {
            None
        };
        let rows = match key_type.length_command() {
            Some(command) if key_type.is_supported() => self
                .run(db, &cmd_value(command, &[key]))
                .await
                .ok()
                .and_then(|value| reply::reply_integer(&value))
                .map(|length| length.max(0) as u64),
            _ => None,
        };
        let encoding = self
            .run(db, &cmd_value("OBJECT", &["ENCODING", key]))
            .await
            .ok()
            .and_then(|value| reply::reply_text(&value));
        // A string key's content is what the key list's 值 column previews; a collection has no
        // single value, so it gets none (the list shows its entry count there instead).
        let preview = match key_type {
            KeyType::String => {
                let last = (PREVIEW_BYTES - 1).to_string();
                self.run(db, &cmd_value("GETRANGE", &[key, "0", &last]))
                    .await
                    .ok()
                    .and_then(|value| reply::reply_text(&value))
                    .filter(|text| !text.is_empty())
            }
            _ => None,
        };
        Ok(TableStatus {
            engine: Some(key_type.label().to_string()),
            rows,
            data_length: memory,
            comment: encoding.clone(),
            ttl,
            preview,
            ..Default::default()
        })
    }

    fn server_version_text(&self) -> String {
        if self.info.flavor == "redis" {
            self.info.version_text.clone()
        } else {
            format!("{} ({})", self.info.version_text, self.info.flavor)
        }
    }
}

enum Handle {
    Single(MultiplexedConnection),
    Cluster(ClusterConnection),
}

/// A live Redis stream: `MONITOR` lines or pub/sub messages.
enum RedisWatch {
    Monitor(Pin<Box<dyn Stream<Item = String> + Send>>),
    PubSub(Pin<Box<dyn Stream<Item = Msg> + Send>>),
}

#[async_trait]
impl Watch for RedisWatch {
    async fn next(&mut self) -> Result<Option<WatchEvent>> {
        match self {
            Self::Monitor(stream) => Ok(stream.next().await.map(|line| WatchEvent {
                channel: "monitor".to_string(),
                text: line,
            })),
            Self::PubSub(stream) => Ok(stream.next().await.map(|message| WatchEvent {
                channel: message.get_channel_name().to_string(),
                text: message.get_payload::<String>().unwrap_or_default(),
            })),
        }
    }
}

async fn connect_client(
    client: &Client,
    connect_timeout: Option<Duration>,
    response_timeout: Option<Duration>,
) -> Result<MultiplexedConnection> {
    let mut config = redis::AsyncConnectionConfig::new();
    if let Some(timeout) = connect_timeout {
        config = config.set_connection_timeout(timeout);
    }
    if let Some(timeout) = response_timeout {
        config = config.set_response_timeout(timeout);
    }
    client
        .get_multiplexed_async_connection_with_config(&config)
        .await
        .map_err(connect_error)
}

async fn open_single(
    setup: &SingleSetup,
    db: i64,
    response_timeout: Option<Duration>,
) -> Result<MultiplexedConnection> {
    let client = setup.client(db)?;
    connect_client(&client, setup.connect_timeout, response_timeout).await
}

async fn read_server_info(connection: &mut MultiplexedConnection) -> ServerInfo {
    let read = |connection: &mut MultiplexedConnection, section: Option<&str>| {
        let mut cmd = Cmd::new();
        cmd.arg("INFO");
        if let Some(section) = section {
            cmd.arg(section);
        }
        let mut connection = connection.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(5), connection.req_packed_command(&cmd))
                .await
                .ok()
                .and_then(|result| result.ok())
                .and_then(|value| reply::reply_text(&value))
        }
    };
    // `INFO <section>` needs Redis 2.6; older servers reject the argument, so retry bare `INFO`.
    let text = match read(connection, Some("server")).await {
        Some(text) => Some(text),
        None => read(connection, None).await,
    };
    text.map(|text| ServerInfo::parse(&text))
        .unwrap_or(ServerInfo {
            version: crate::version::ServerVersion::UNKNOWN,
            version_text: String::new(),
            flavor: "redis".to_string(),
        })
}

async fn read_server_info_cluster(cluster: &ClusterConnection) -> ServerInfo {
    let mut cluster = cluster.clone();
    match cluster
        .req_packed_command(&cmd_value("INFO", &["server"]))
        .await
    {
        Ok(value) => reply::reply_text(&value)
            .map(|text| ServerInfo::parse(&text))
            .unwrap_or(ServerInfo {
                version: crate::version::ServerVersion::UNKNOWN,
                version_text: String::new(),
                flavor: "redis".to_string(),
            }),
        Err(_) => ServerInfo {
            version: crate::version::ServerVersion::UNKNOWN,
            version_text: String::new(),
            flavor: "redis".to_string(),
        },
    }
}

/// Scans one connection until the cursor closes or `limit` keys are collected.
async fn scan_connection(
    connection: &mut MultiplexedConnection,
    limit: usize,
    pattern: Option<&str>,
) -> Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut cursor = "0".to_string();
    loop {
        let mut cmd = cmd_value("SCAN", &[cursor.as_str()]);
        cmd.arg("MATCH")
            .arg(pattern.unwrap_or("*"))
            .arg("COUNT")
            .arg(SCAN_COUNT);
        let value = connection
            .req_packed_command(&cmd)
            .await
            .map_err(query_error)?;
        let Value::Array(parts) = value else {
            return Err(Error::Query("unexpected SCAN reply".to_string()));
        };
        if let Some(next) = parts.first().and_then(reply::reply_text) {
            cursor = next;
        }
        if let Some(Value::Array(batch)) = parts.get(1) {
            for key in batch {
                if let Some(key) = reply::reply_text(key) {
                    keys.push(key);
                }
            }
        }
        if cursor == "0" || keys.len() >= limit {
            break;
        }
    }
    Ok(keys)
}

impl RedisConnection {
    /// Reads one page of a key's content as rows.
    async fn fetch_key_page(&self, db: i64, key: &str, page: PageRequest) -> Result<TablePage> {
        let key_type = self.key_type(db, key).await?;
        let Some(columns) = key_type.columns() else {
            return Err(Error::Query(format!(
                "keys of type {} are not supported",
                key_type.label()
            )));
        };
        let page_number = page.page.max(1);
        let page_size = page.page_size.max(1);
        let offset = (page_number - 1) * page_size;
        let offset_usize = offset as usize;
        let page_size_usize = page_size as usize;

        let (rows, total) = match &key_type {
            KeyType::String => {
                let value = self.run(db, &cmd_value("GET", &[key])).await?;
                let cell = reply::cell(&value);
                (vec![vec![cell]], Some(1))
            }
            KeyType::Json => {
                let value = self.run(db, &cmd_value("JSON.GET", &[key])).await?;
                (vec![vec![reply::cell(&value)]], Some(1))
            }
            KeyType::List => {
                let total = self.total_of(db, key, "LLEN").await?;
                let start = offset as i64;
                let stop = (offset + page_size) as i64 - 1;
                let mut cmd = cmd_value("LRANGE", &[key]);
                cmd.arg(start).arg(stop);
                let value = self.run(db, &cmd).await?;
                let rows = match value {
                    Value::Array(items) => items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            vec![CellValue::Int(start + index as i64), reply::cell(item)]
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                (rows, Some(total))
            }
            KeyType::ZSet => {
                let total = self.total_of(db, key, "ZCARD").await?;
                let start = offset as i64;
                let stop = (offset + page_size) as i64 - 1;
                let mut cmd = cmd_value("ZRANGE", &[key]);
                cmd.arg(start).arg(stop).arg("WITHSCORES");
                let value = self.run(db, &cmd).await?;
                let rows = pairs_from_array(&value)
                    .into_iter()
                    .map(|(member, score)| {
                        vec![
                            CellValue::Text(member),
                            score
                                .parse::<f64>()
                                .map(CellValue::Float)
                                .unwrap_or(CellValue::Text(score)),
                        ]
                    })
                    .collect();
                (rows, Some(total))
            }
            KeyType::Hash => {
                let total = self.total_of(db, key, "HLEN").await?;
                if total > COLLECTION_FETCH_LIMIT {
                    return Err(collection_too_large(key_type.label(), total));
                }
                let value = self.run(db, &cmd_value("HGETALL", &[key])).await?;
                let mut pairs = pairs_from_array(&value);
                pairs.sort_by(|left, right| left.0.cmp(&right.0));
                pairs = apply_order(pairs, &page.order_by);
                let rows = pairs
                    .into_iter()
                    .skip(offset_usize)
                    .take(page_size_usize)
                    .map(|(field, value)| vec![CellValue::Text(field), CellValue::Text(value)])
                    .collect();
                (rows, Some(total))
            }
            KeyType::Set => {
                let total = self.total_of(db, key, "SCARD").await?;
                if total > COLLECTION_FETCH_LIMIT {
                    return Err(collection_too_large(key_type.label(), total));
                }
                let value = self.run(db, &cmd_value("SMEMBERS", &[key])).await?;
                let mut members: Vec<String> = match value {
                    Value::Array(items) | Value::Set(items) => {
                        items.iter().filter_map(reply::reply_text).collect()
                    }
                    _ => Vec::new(),
                };
                members.sort();
                if page.order_by.first().is_some_and(|sort| sort.descending) {
                    members.reverse();
                }
                let rows = members
                    .into_iter()
                    .skip(offset_usize)
                    .take(page_size_usize)
                    .map(|member| vec![CellValue::Text(member)])
                    .collect();
                (rows, Some(total))
            }
            KeyType::Stream => {
                let total = self.total_of(db, key, "XLEN").await?;
                if total > COLLECTION_FETCH_LIMIT {
                    return Err(collection_too_large(key_type.label(), total));
                }
                let mut cmd = cmd_value("XRANGE", &[key]);
                cmd.arg("-").arg("+").arg("COUNT").arg(total.max(1));
                let value = self.run(db, &cmd).await?;
                let entries = stream_entries(&value);
                let rows = entries
                    .into_iter()
                    .skip(offset_usize)
                    .take(page_size_usize)
                    .flat_map(|(id, fields)| {
                        fields.into_iter().map(move |(field, value)| {
                            vec![
                                CellValue::Text(id.clone()),
                                CellValue::Text(field),
                                CellValue::Text(value),
                            ]
                        })
                    })
                    .collect();
                (rows, Some(total))
            }
            KeyType::Unknown(_) => (Vec::new(), Some(0)),
        };

        Ok(TablePage {
            columns,
            rows,
            page: page_number,
            page_size,
            total_rows: total,
        })
    }

    async fn total_of(&self, db: i64, key: &str, command: &str) -> Result<u64> {
        let value = self.run(db, &cmd_value(command, &[key])).await?;
        Ok(reply::reply_integer(&value).unwrap_or(0).max(0) as u64)
    }
}

#[async_trait]
impl Connection for RedisConnection {
    fn driver_id(&self) -> DriverId {
        DriverId::new("redis")
    }

    async fn list_databases(&self) -> Result<Vec<DatabaseInfo>> {
        if self.is_cluster() {
            return Ok(vec![DatabaseInfo {
                name: "db0".to_string(),
            }]);
        }
        let configured = self
            .run(0, &cmd_value("CONFIG", &["GET", "databases"]))
            .await
            .ok()
            .and_then(|value| {
                let mut parts = match value {
                    Value::Array(parts) => parts,
                    _ => Vec::new(),
                };
                if parts.len() >= 2 {
                    reply::reply_integer(&parts.remove(1))
                } else {
                    None
                }
            })
            .unwrap_or(16)
            .clamp(1, 1024);
        // Databases that hold keys are listed even when `CONFIG` is disabled or lies.
        let mut count = configured;
        if let Ok(value) = self.run(0, &cmd_value("INFO", &["keyspace"])).await
            && let Some(text) = reply::reply_text(&value)
        {
            count = count.max(keyspace_database_count(&text));
        }
        Ok((0..count)
            .map(|index| DatabaseInfo {
                name: format!("db{index}"),
            })
            .collect())
    }

    async fn list_tables(&self, database: &str) -> Result<Vec<TableInfo>> {
        let db = self.db_for(Some(database))?;
        let keys = self.scan_keys(db, self.key_limit, None).await?;
        Ok(keys
            .into_iter()
            .map(|name| TableInfo {
                name,
                kind: ObjectKind::Table,
                updatable: false,
            })
            .collect())
    }

    async fn columns(&self, database: &str, table: &str) -> Result<Vec<ColumnInfo>> {
        let db = self.db_for(Some(database))?;
        let key_type = self.key_type(db, table).await?;
        key_type.columns().ok_or_else(|| {
            Error::Query(format!(
                "keys of type {} are not supported",
                key_type.label()
            ))
        })
    }

    async fn fetch_page(
        &self,
        database: &str,
        table: &str,
        page: PageRequest,
    ) -> Result<TablePage> {
        let db = self.db_for(Some(database))?;
        self.fetch_key_page(db, table, page).await
    }

    async fn update_rows(&self, database: &str, table: &str, updates: &[RowUpdate]) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        let key_type = self.key_type(db, table).await?;
        for update in updates {
            let set = |column: &str| {
                update
                    .set
                    .iter()
                    .find(|(name, _)| name == column)
                    .and_then(|(_, value)| value.clone())
            };
            let key = |column: &str| {
                update
                    .keys
                    .iter()
                    .find(|(name, _)| name == column)
                    .and_then(|(_, value)| value.clone())
            };
            match &key_type {
                KeyType::String => {
                    if let Some(value) = set("value") {
                        self.set_string(db, table, &value).await?;
                    }
                }
                KeyType::List => {
                    let (Some(index), Some(value)) = (key("index"), set("value")) else {
                        continue;
                    };
                    let index: i64 = index
                        .parse()
                        .map_err(|_| Error::Query(format!("invalid list index {index}")))?;
                    let mut cmd = cmd_value("LSET", &[table]);
                    cmd.arg(index).arg(value);
                    self.run(db, &cmd).await?;
                }
                KeyType::Hash => {
                    let Some(field) = key("field") else { continue };
                    let value = set("value").unwrap_or_default();
                    if let Some(new_field) = set("field")
                        && new_field != field
                    {
                        self.run(db, &cmd_value("HDEL", &[table, field.as_str()]))
                            .await?;
                        let mut cmd = cmd_value("HSET", &[table]);
                        cmd.arg(new_field).arg(value);
                        self.run(db, &cmd).await?;
                    } else {
                        let mut cmd = cmd_value("HSET", &[table]);
                        cmd.arg(field).arg(value);
                        self.run(db, &cmd).await?;
                    }
                }
                KeyType::Set => {
                    let Some(member) = key("member") else {
                        continue;
                    };
                    if let Some(new_member) = set("member")
                        && new_member != member
                    {
                        self.run(db, &cmd_value("SREM", &[table, member.as_str()]))
                            .await?;
                        self.run(db, &cmd_value("SADD", &[table, new_member.as_str()]))
                            .await?;
                    }
                }
                KeyType::ZSet => {
                    let Some(member) = key("member") else {
                        continue;
                    };
                    let score = set("score")
                        .filter(|score| !score.trim().is_empty())
                        .unwrap_or_else(|| "0".to_string());
                    let new_member = set("member").unwrap_or_else(|| member.clone());
                    if new_member != member {
                        self.run(db, &cmd_value("ZREM", &[table, member.as_str()]))
                            .await?;
                    }
                    let mut cmd = cmd_value("ZADD", &[table]);
                    cmd.arg(score).arg(new_member);
                    self.run(db, &cmd).await?;
                }
                KeyType::Stream | KeyType::Json | KeyType::Unknown(_) => {
                    return Err(Error::Query(format!(
                        "keys of type {} are read-only",
                        key_type.label()
                    )));
                }
            }
        }
        Ok(())
    }

    async fn insert_rows(&self, database: &str, table: &str, rows: &[RowInsert]) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        let key_type = self.key_type(db, table).await?;
        for row in rows {
            let value = |column: &str| {
                row.values
                    .iter()
                    .find(|(name, _)| name == column)
                    .and_then(|(_, value)| value.clone())
            };
            match &key_type {
                KeyType::String => {
                    if let Some(value) = value("value") {
                        self.set_string(db, table, &value).await?;
                    }
                }
                KeyType::Json => {
                    return Err(Error::Query(
                        "json keys are edited through the console (JSON.SET)".to_string(),
                    ));
                }
                KeyType::List => {
                    let value = value("value").unwrap_or_default();
                    let mut cmd = cmd_value("RPUSH", &[table]);
                    cmd.arg(value);
                    self.run(db, &cmd).await?;
                }
                KeyType::Set => {
                    let member = value("member").unwrap_or_default();
                    let mut cmd = cmd_value("SADD", &[table]);
                    cmd.arg(member);
                    self.run(db, &cmd).await?;
                }
                KeyType::Hash => {
                    let field = value("field").unwrap_or_default();
                    let value = value("value").unwrap_or_default();
                    let mut cmd = cmd_value("HSET", &[table]);
                    cmd.arg(field).arg(value);
                    self.run(db, &cmd).await?;
                }
                KeyType::ZSet => {
                    let member = value("member").unwrap_or_default();
                    let score = value("score")
                        .filter(|score| !score.trim().is_empty())
                        .unwrap_or_else(|| "0".to_string());
                    let mut cmd = cmd_value("ZADD", &[table]);
                    cmd.arg(score).arg(member);
                    self.run(db, &cmd).await?;
                }
                KeyType::Stream | KeyType::Unknown(_) => {
                    return Err(Error::Query(format!(
                        "keys of type {} are read-only",
                        key_type.label()
                    )));
                }
            }
        }
        Ok(())
    }

    async fn delete_rows(
        &self,
        database: &str,
        table: &str,
        keys: &[Vec<(String, Option<String>)>],
    ) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        let key_type = self.key_type(db, table).await?;
        match &key_type {
            KeyType::String | KeyType::Json => {
                self.drop_key(db, table).await?;
            }
            KeyType::List => {
                let mut indices: Vec<i64> = keys
                    .iter()
                    .filter_map(|row| row_value(row, "index"))
                    .filter_map(|index| index.parse().ok())
                    .collect();
                indices.sort_unstable_by(|left, right| right.cmp(left));
                let marker = format!("__rustgrid_deleted_{}__", std::process::id());
                for index in indices {
                    let mut cmd = cmd_value("LSET", &[table]);
                    cmd.arg(index).arg(&marker);
                    self.run(db, &cmd).await?;
                    let mut cmd = cmd_value("LREM", &[table]);
                    cmd.arg(0).arg(&marker);
                    self.run(db, &cmd).await?;
                }
            }
            KeyType::Hash => {
                let mut cmd = cmd_value("HDEL", &[table]);
                for field in keys.iter().filter_map(|row| row_value(row, "field")) {
                    cmd.arg(field);
                }
                self.run(db, &cmd).await?;
            }
            KeyType::Set => {
                let mut cmd = cmd_value("SREM", &[table]);
                for member in keys.iter().filter_map(|row| row_value(row, "member")) {
                    cmd.arg(member);
                }
                self.run(db, &cmd).await?;
            }
            KeyType::ZSet => {
                let mut cmd = cmd_value("ZREM", &[table]);
                for member in keys.iter().filter_map(|row| row_value(row, "member")) {
                    cmd.arg(member);
                }
                self.run(db, &cmd).await?;
            }
            KeyType::Stream | KeyType::Unknown(_) => {
                return Err(Error::Query(format!(
                    "keys of type {} are read-only",
                    key_type.label()
                )));
            }
        }
        Ok(())
    }

    async fn execute_query(&self, database: Option<&str>, sql: &str) -> Result<QueryResult> {
        let mut results = self.execute_query_many(database, sql).await?;
        results
            .pop()
            .ok_or_else(|| Error::Query("empty command".to_string()))
    }

    async fn execute_query_many(
        &self,
        database: Option<&str>,
        sql: &str,
    ) -> Result<Vec<QueryResult>> {
        let db = self.db_for(database)?;
        let lines = command::split_script(sql);
        if lines.is_empty() {
            return Err(Error::Query("empty command".to_string()));
        }
        let mut results = Vec::with_capacity(lines.len());
        for line in lines {
            results.push(self.run_line(db, &line).await?);
        }
        Ok(results)
    }

    fn split_statements(&self, sql: &str) -> Vec<String> {
        command::split_script(sql)
    }

    async fn create_database(&self, name: &str, _options: &DatabaseOptions) -> Result<()> {
        Err(Error::Query(format!(
            "Redis databases are numbered and configured by the server; {name} cannot be created"
        )))
    }

    fn create_database_sql(&self, _name: &str, _options: &DatabaseOptions) -> String {
        String::new()
    }

    async fn drop_database(&self, name: &str) -> Result<()> {
        Err(Error::Query(format!(
            "Redis databases are numbered and configured by the server; {name} cannot be dropped"
        )))
    }

    async fn drop_table(&self, database: &str, table: &str) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        self.drop_key(db, table).await
    }

    async fn empty_table(&self, database: &str, table: &str) -> Result<()> {
        // An empty collection does not exist in Redis; removing every entry removes the key.
        self.truncate_table(database, table).await
    }

    async fn truncate_table(&self, database: &str, table: &str) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        self.drop_key(db, table).await
    }

    async fn rename_table(&self, database: &str, table: &str, new_name: &str) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        let mut cmd = cmd_value("RENAME", &[table]);
        cmd.arg(new_name);
        self.run(db, &cmd).await?;
        Ok(())
    }

    async fn database_options(&self, _name: &str) -> Result<DatabaseOptions> {
        Ok(DatabaseOptions::default())
    }

    async fn server_version(&self) -> Result<String> {
        Ok(self.server_version_text())
    }

    async fn session_count(&self) -> Result<u64> {
        let value = self.run(0, &cmd_value("INFO", &["clients"])).await?;
        let text = reply::reply_text(&value).unwrap_or_default();
        Ok(text
            .lines()
            .find_map(|line| line.strip_prefix("connected_clients:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0))
    }

    async fn table_status(&self, database: &str, table: &str) -> Result<TableStatus> {
        let db = self.db_for(Some(database))?;
        self.status_for(db, table).await
    }

    async fn table_statuses(&self, database: &str) -> Result<Vec<(String, TableStatus)>> {
        let db = self.db_for(Some(database))?;
        let keys = self.scan_keys(db, self.key_limit, None).await?;
        let statuses = stream::iter(keys.into_iter().map(|key| async move {
            let status = self.status_for(db, &key).await.unwrap_or_default();
            (key, status)
        }))
        .buffer_unordered(STATUS_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        Ok(statuses)
    }

    async fn list_objects_page(
        &self,
        database: &str,
        _cursor: Option<&str>,
        pattern: Option<&str>,
    ) -> Result<(Vec<TableInfo>, Option<String>)> {
        let db = self.db_for(Some(database))?;
        let pattern = pattern.map(str::trim).filter(|pattern| !pattern.is_empty());
        // Load the connection's configured key limit in one go (the 加载更多 pager was removed):
        // `scan_keys` follows the SCAN cursor internally until the limit is reached or the whole
        // keyspace was walked. A cluster has no cross-node cursor, and this path is the same
        // bounded snapshot for it.
        let keys = self.scan_keys(db, self.key_limit, pattern).await?;
        Ok((keys.into_iter().map(table_info).collect(), None))
    }

    async fn list_namespaces(&self, database: &str) -> Result<Vec<ObjectNamespace>> {
        let db = self.db_for(Some(database))?;
        let keys = self.scan_keys(db, self.key_limit, None).await?;
        let mut namespaces: std::collections::BTreeMap<String, u64> =
            std::collections::BTreeMap::new();
        for key in keys {
            let name = match key.find(':') {
                Some(index) => key[..=index].to_string(),
                None => String::new(),
            };
            *namespaces.entry(name).or_default() += 1;
        }
        Ok(namespaces
            .into_iter()
            .map(|(name, objects)| ObjectNamespace { name, objects })
            .collect())
    }

    async fn create_object(&self, database: &str, name: &str, spec: &ObjectCreate) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        if name.trim().is_empty() {
            return Err(Error::Query("the key name is required".to_string()));
        }
        let value = |column: &str| {
            spec.values
                .iter()
                .find(|(field, _)| field == column)
                .and_then(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        match spec.kind.trim().to_ascii_lowercase().as_str() {
            "string" => {
                let mut cmd = cmd_value("SET", &[name]);
                cmd.arg(value("value"));
                self.run(db, &cmd).await?;
            }
            "hash" => {
                let mut cmd = cmd_value("HSET", &[name]);
                cmd.arg(value("field")).arg(value("value"));
                self.run(db, &cmd).await?;
            }
            "list" => {
                let mut cmd = cmd_value("RPUSH", &[name]);
                cmd.arg(value("value"));
                self.run(db, &cmd).await?;
            }
            "set" => {
                let mut cmd = cmd_value("SADD", &[name]);
                cmd.arg(value("value"));
                self.run(db, &cmd).await?;
            }
            "zset" => {
                let score = value("score");
                let score = if score.trim().is_empty() { "0" } else { &score };
                let mut cmd = cmd_value("ZADD", &[name]);
                cmd.arg(score).arg(value("member"));
                self.run(db, &cmd).await?;
            }
            "stream" => {
                let mut cmd = cmd_value("XADD", &[name, "*"]);
                cmd.arg(value("field")).arg(value("value"));
                self.run(db, &cmd).await?;
            }
            other => {
                return Err(Error::Query(format!(
                    "{other} is not a Redis type this driver can create"
                )));
            }
        }
        if let Some(ttl) = spec.ttl.filter(|ttl| *ttl >= 0) {
            let mut expire = cmd_value("EXPIRE", &[name]);
            if ttl == 0 {
                // 0 removes the key immediately, which is not what "create with TTL 0" means.
                let _ = self.run(db, &cmd_value("PERSIST", &[name])).await;
            } else {
                expire.arg(ttl);
                self.run(db, &expire).await?;
            }
        }
        Ok(())
    }

    async fn object_ttl(&self, database: &str, name: &str) -> Result<Option<i64>> {
        let db = self.db_for(Some(database))?;
        let value = self.run(db, &cmd_value("TTL", &[name])).await?;
        match reply::reply_integer(&value) {
            Some(-2) => Err(Error::Query("the key does not exist".to_string())),
            Some(ttl) => Ok(Some(ttl)),
            None => Ok(None),
        }
    }

    async fn set_object_ttl(&self, database: &str, name: &str, ttl: Option<i64>) -> Result<()> {
        let db = self.db_for(Some(database))?;
        self.ensure_writable()?;
        match ttl {
            Some(seconds) if seconds >= 0 => {
                let mut cmd = cmd_value("EXPIRE", &[name]);
                cmd.arg(seconds);
                self.run(db, &cmd).await?;
            }
            _ => {
                self.run(db, &cmd_value("PERSIST", &[name])).await?;
            }
        }
        Ok(())
    }

    async fn watch(
        &self,
        database: Option<&str>,
        spec: &WatchSpec,
    ) -> Result<Option<Box<dyn Watch>>> {
        let db = self.db_for(database)?;
        let client = self.stream_client(db).await?;
        match spec {
            WatchSpec::Monitor => {
                let monitor = client.get_async_monitor().await.map_err(query_error)?;
                Ok(Some(Box::new(RedisWatch::Monitor(Box::pin(
                    monitor.into_on_message::<String>(),
                )))))
            }
            WatchSpec::Subscribe { channels, patterns } => {
                let mut pubsub = client.get_async_pubsub().await.map_err(query_error)?;
                for channel in channels {
                    pubsub
                        .subscribe(channel.clone())
                        .await
                        .map_err(query_error)?;
                }
                for pattern in patterns {
                    pubsub
                        .psubscribe(pattern.clone())
                        .await
                        .map_err(query_error)?;
                }
                Ok(Some(Box::new(RedisWatch::PubSub(Box::pin(
                    pubsub.into_on_message(),
                )))))
            }
        }
    }

    async fn object_ddl(
        &self,
        _database: &str,
        _name: &str,
        _is_view: bool,
    ) -> Result<Option<String>> {
        Ok(None)
    }

    async fn table_schema(&self, _database: &str, _table: &str) -> Result<TableSchema> {
        Err(Error::Query(
            "Redis keys have no table schema; a key's type cannot be altered".to_string(),
        ))
    }

    fn table_schema_sql(
        &self,
        _database: &str,
        _table: &str,
        _original: Option<&TableSchema>,
        _modified: &TableSchema,
    ) -> String {
        String::new()
    }

    async fn close(&self) -> Result<()> {
        self.connections.lock().await.clear();
        Ok(())
    }

    async fn character_sets(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn collations(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn alter_database_options(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> Result<()> {
        Err(Error::Query(
            "Redis databases have no editable options".to_string(),
        ))
    }

    fn alter_database_sql(
        &self,
        _name: &str,
        _original: &DatabaseOptions,
        _modified: &DatabaseOptions,
    ) -> String {
        String::new()
    }

    fn column_types(&self) -> Vec<&'static str> {
        Vec::new()
    }

    async fn list_routines(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn list_events(&self, _database: &str) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    async fn backup_object_metadata(
        &self,
        _database: &str,
        _kind: BackupObjectKind,
        _name: &str,
    ) -> Result<ObjectDump> {
        Err(Error::Query(
            "Redis keys are not backed up through the SQL backup pipeline".to_string(),
        ))
    }

    async fn stream_table_rows(
        &self,
        _database: &str,
        _table: &str,
        _on_row: &mut (dyn for<'a> FnMut(&'a str) -> Result<()> + Send),
    ) -> Result<u64> {
        Err(Error::Query(
            "Redis keys are not streamed through the SQL backup pipeline".to_string(),
        ))
    }

    async fn restore_object(&self, _database: &str, _object: &ObjectDump) -> Result<()> {
        Err(Error::Query(
            "Redis keys are not restored through the SQL backup pipeline".to_string(),
        ))
    }

    fn storage_engines(&self) -> Vec<&'static str> {
        Vec::new()
    }
}

impl RedisConnection {
    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            return Err(Error::Query(
                "this connection is read-only (turn off Read-Only mode to edit)".to_string(),
            ));
        }
        Ok(())
    }

    /// A plain client for the live streams: standalone uses its own setup, sentinel discovers the
    /// master through `SENTINEL`, and a cluster uses its first master (a `MONITOR` stream is
    /// per-node; the UI says so).
    async fn stream_client(&self, db: i64) -> Result<Client> {
        match &self.mode {
            Mode::Standalone => self
                .single
                .as_ref()
                .ok_or_else(|| Error::Connection("the connection has no client".to_string()))?
                .client(db),
            Mode::Sentinel(setup) => {
                let reply = self
                    .run(
                        0,
                        &cmd_value(
                            "SENTINEL",
                            &["get-master-addr-by-name", setup.master.as_str()],
                        ),
                    )
                    .await?;
                let Value::Array(parts) = reply else {
                    return Err(Error::Query(format!(
                        "the sentinels do not know the master {}",
                        setup.master
                    )));
                };
                let host = parts
                    .first()
                    .and_then(reply::reply_text)
                    .ok_or_else(|| Error::Query("the sentinel returned no master".to_string()))?;
                let port = parts
                    .get(1)
                    .and_then(reply::reply_integer)
                    .ok_or_else(|| Error::Query("the sentinel returned no port".to_string()))?
                    as u16;
                setup.client_for(&host, port, db)
            }
            Mode::Cluster => {
                let masters = self.cluster_masters().await?;
                let (host, port) = masters
                    .first()
                    .cloned()
                    .ok_or_else(|| Error::Query("the cluster has no master node".to_string()))?;
                self.node_setup
                    .as_ref()
                    .ok_or_else(|| Error::Connection("the connection has no client".to_string()))?
                    .client_for(&host, port, 0)
            }
        }
    }

    async fn drop_key(&self, db: i64, key: &str) -> Result<()> {
        let command = if self.info.version.has_unlink() {
            "UNLINK"
        } else {
            "DEL"
        };
        self.run(db, &cmd_value(command, &[key])).await?;
        Ok(())
    }

    async fn set_string(&self, db: i64, key: &str, value: &str) -> Result<()> {
        let mut cmd = redis::Cmd::new();
        cmd.arg("SET").arg(key).arg(value);
        if self.info.version.has_keep_ttl() {
            cmd.arg("KEEPTTL");
        } else {
            // Redis < 6 has no KEEPTTL: read the remaining TTL and restore it around the write so
            // editing a value does not silently make an expiring key permanent.
            let ttl = self
                .run(db, &cmd_value("PTTL", &[key]))
                .await
                .ok()
                .and_then(|value| reply::reply_integer(&value))
                .filter(|ttl| *ttl > 0);
            self.run(db, &cmd).await?;
            if let Some(ttl) = ttl {
                let mut expire = cmd_value("PEXPIRE", &[key]);
                expire.arg(ttl);
                self.run(db, &expire).await?;
            }
            return Ok(());
        }
        self.run(db, &cmd).await?;
        Ok(())
    }

    async fn run_line(&self, db: i64, line: &str) -> Result<QueryResult> {
        let parsed = command::parse_args(line).map_err(Error::Query)?;
        let name = parsed.name_lower();
        if command::stateful_command(&name) {
            return Err(Error::Query(format!(
                "{} changes the shared connection state and is not available in the query editor",
                parsed.name
            )));
        }
        if name == "client"
            && let Some(subcommand) = parsed.args.first()
            && command::stateful_client_subcommand(&subcommand.to_ascii_lowercase())
        {
            return Err(Error::Query(format!(
                "CLIENT {subcommand} changes the shared connection state and is not available in \
                 the query editor"
            )));
        }
        if command::blocking_command(&name) {
            return Err(Error::Query(format!(
                "{} blocks the connection and is not available in the query editor",
                parsed.name
            )));
        }
        if command::destructive_command(&name) {
            return Err(Error::Query(format!(
                "{} is not allowed in the query editor",
                parsed.name
            )));
        }
        if self.read_only && command::write_command(&name, &parsed.args) {
            return Err(Error::Query(
                "this connection is read-only (turn off Read-Only mode to run writes)".to_string(),
            ));
        }
        if name == "subscribe" || name == "psubscribe" || name == "ssubscribe" {
            return Err(Error::Query(
                "subscriptions are not available in the query editor".to_string(),
            ));
        }
        let mut cmd = Cmd::new();
        cmd.arg(&parsed.name);
        for argument in &parsed.args {
            cmd.arg(argument);
        }
        let value = self.run(db, &cmd).await?;
        let pairs = name == "hgetall"
            || name == "pubsub"
                && parsed.args.first().is_some_and(|argument| {
                    argument.eq_ignore_ascii_case("numsub")
                        || argument.eq_ignore_ascii_case("shardnumsub")
                })
            || name == "config"
                && parsed
                    .args
                    .first()
                    .is_some_and(|argument| argument.eq_ignore_ascii_case("get"));
        Ok(if pairs {
            reply::pairs_result(line, &value)
        } else {
            reply::reply_result(line, &value)
        })
    }
}

/// Builds one command from a name and its fixed arguments.
/// One key as an object-list entry.
fn table_info(name: String) -> TableInfo {
    TableInfo {
        name,
        kind: ObjectKind::Table,
        updatable: false,
    }
}

/// Builds one command from a name and its fixed arguments.
fn cmd_value(name: &str, args: &[&str]) -> Cmd {
    let mut cmd = Cmd::new();
    cmd.arg(name);
    for argument in args {
        cmd.arg(*argument);
    }
    cmd
}

/// Maps a `TYPE`d key's flat reply into `(member, score)` pairs.
fn pairs_from_array(value: &Value) -> Vec<(String, String)> {
    let items = match value {
        Value::Array(items) | Value::Set(items) => items.clone(),
        Value::Map(pairs) => {
            return pairs
                .iter()
                .filter_map(|(key, value)| {
                    Some((reply::reply_text(key)?, reply::reply_text(value)?))
                })
                .collect();
        }
        _ => Vec::new(),
    };
    items
        .chunks(2)
        .filter_map(|pair| {
            let key = pair.first().and_then(reply::reply_text)?;
            let value = pair.get(1).and_then(reply::reply_text).unwrap_or_default();
            Some((key, value))
        })
        .collect()
}

/// Expands an `XRANGE` reply into `(entry id, fields)`.
fn stream_entries(value: &Value) -> Vec<(String, Vec<(String, String)>)> {
    let Value::Array(entries) = value else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let Value::Array(parts) = entry else {
                return None;
            };
            let id = parts.first().and_then(reply::reply_text)?;
            let Value::Array(fields) = parts.get(1)? else {
                return None;
            };
            let fields = fields
                .chunks(2)
                .filter_map(|pair| {
                    Some((
                        pair.first().and_then(reply::reply_text)?,
                        pair.get(1).and_then(reply::reply_text).unwrap_or_default(),
                    ))
                })
                .collect();
            Some((id, fields))
        })
        .collect()
}

fn apply_order(
    mut pairs: Vec<(String, String)>,
    order_by: &[rustgrid_core::SortColumn],
) -> Vec<(String, String)> {
    if let Some(sort) = order_by.first() {
        match sort.column.as_str() {
            "field" => pairs.sort_by(|left, right| left.0.cmp(&right.0)),
            "value" => pairs.sort_by(|left, right| left.1.cmp(&right.1)),
            _ => {}
        }
        if sort.descending {
            pairs.reverse();
        }
    }
    pairs
}

fn collection_too_large(kind: &str, count: u64) -> Error {
    Error::Query(format!(
        "this {kind} holds {count} entries; RustGrid shows at most {COLLECTION_FETCH_LIMIT}. Use a \
         *SCAN command in a query tab to page through it."
    ))
}

fn row_value(row: &[(String, Option<String>)], column: &str) -> Option<String> {
    row.iter()
        .find(|(name, _)| name == column)
        .and_then(|(_, value)| value.clone())
}

fn connection_info(
    host: &str,
    port: u16,
    db: i64,
    tls: &TlsPlan,
    username: &Option<String>,
    password: &Option<String>,
) -> ConnectionInfo {
    let addr = if tls.enabled {
        ConnectionAddr::TcpTls {
            host: host.to_string(),
            port,
            insecure: tls.insecure,
            tls_params: None,
        }
    } else {
        ConnectionAddr::Tcp(host.to_string(), port)
    };
    ConnectionInfo {
        addr,
        redis: RedisConnectionInfo {
            db,
            username: username.clone(),
            password: password.clone(),
            protocol: ProtocolVersion::RESP2,
        },
    }
}

impl TlsPlan {
    fn resolve(settings: &rustgrid_core::ConnectionOptions) -> Result<Self> {
        use rustgrid_core::TlsMode;
        let tls = &settings.tls;
        if tls.mode == TlsMode::Disabled {
            return Ok(Self::default());
        }
        let read = |path: &str| -> Result<Option<Vec<u8>>> {
            let path = path.trim();
            if path.is_empty() {
                return Ok(None);
            }
            std::fs::read(path)
                .map(Some)
                .map_err(|error| Error::Config(format!("could not read {path}: {error}")))
        };
        let root = read(&tls.ca)?;
        let cert = read(&tls.cert)?;
        let key = read(&tls.key)?;
        let client_tls = match (cert, key) {
            (Some(cert), Some(key)) => Some(redis::ClientTlsConfig {
                client_cert: cert,
                client_key: key,
            }),
            (None, None) => None,
            _ => {
                return Err(Error::Config(
                    "a client certificate and its key must be provided together".to_string(),
                ));
            }
        };
        let certs = if root.is_some() || client_tls.is_some() {
            Some(TlsCertificates {
                client_tls,
                root_cert: root,
            })
        } else {
            None
        };
        Ok(Self {
            enabled: true,
            insecure: matches!(tls.mode, TlsMode::Preferred | TlsMode::Required),
            plain_fallback: tls.mode == TlsMode::Preferred,
            certs,
        })
    }
}

/// The number of databases implied by an `INFO keyspace` payload (`db0:keys=…` lines), falling
/// back to 1 when no database has keys.
fn keyspace_database_count(text: &str) -> i64 {
    let mut highest: i64 = -1;
    for line in text.lines() {
        // `db0:keys=3,expires=0,avg_ttl=0` (section headers start with `#`).
        if let Some(rest) = line.strip_prefix("db")
            && let Some((index, _)) = rest.split_once(':')
            && let Ok(index) = index.parse::<i64>()
        {
            highest = highest.max(index);
        }
    }
    highest + 1
}

fn parse_db(value: Option<&str>) -> Result<i64> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(0);
    };
    let digits = value.strip_prefix("db").unwrap_or(value);
    let db: i64 = digits
        .parse()
        .map_err(|_| Error::Config(format!("invalid Redis database {value}")))?;
    if !(0..=1024).contains(&db) {
        return Err(Error::Config(format!(
            "the Redis database must be between 0 and 1024, got {value}"
        )));
    }
    Ok(db)
}

fn parse_key_limit(value: Option<&String>) -> Result<usize> {
    let Some(value) = value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(DEFAULT_KEY_LIMIT);
    };
    let limit: usize = value
        .parse()
        .map_err(|_| Error::Config(format!("invalid key-list limit {value}")))?;
    if limit == 0 {
        return Err(Error::Config(
            "the key-list limit must be at least 1".to_string(),
        ));
    }
    Ok(limit.min(200_000))
}

/// Parses `host:port,host:port` (or a single `host`), falling back to `bare_port`.
fn parse_nodes(
    value: Option<&String>,
    default_port: u16,
    bare_port: u16,
) -> Result<Vec<(String, u16)>> {
    let Some(value) = value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
    else {
        return Ok(Vec::new());
    };
    let mut nodes = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (host, port) = match entry.rsplit_once(':') {
            // A bare IPv6 address contains colons too; a port is all digits after the last colon.
            Some((host, port)) if port.chars().all(|character| character.is_ascii_digit()) => (
                host.trim_matches(['[', ']']).to_string(),
                port.parse()
                    .map_err(|_| Error::Config(format!("invalid port in {entry}")))?,
            ),
            _ => (entry.to_string(), bare_port),
        };
        if host.is_empty() {
            return Err(Error::Config(format!("invalid address {entry}")));
        }
        nodes.push((host, if port == 0 { default_port } else { port }));
    }
    Ok(nodes)
}

fn text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn connect_error(error: RedisError) -> Error {
    if is_auth_error(&error) {
        Error::Authentication(error.to_string())
    } else {
        Error::Connection(error.to_string())
    }
}

fn query_error(error: RedisError) -> Error {
    if is_auth_error(&error) {
        Error::Authentication(error.to_string())
    } else {
        Error::Query(error.to_string())
    }
}

fn is_auth_error(error: &RedisError) -> bool {
    if error.kind() == ErrorKind::AuthenticationFailed {
        return true;
    }
    let message = error.to_string().to_ascii_uppercase();
    message.contains("NOAUTH")
        || message.contains("WRONGPASS")
        || message.contains("AUTHENTICATION")
        || message.contains("INVALID PASSWORD")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_database_names() {
        assert_eq!(parse_db(None).unwrap(), 0);
        assert_eq!(parse_db(Some("")).unwrap(), 0);
        assert_eq!(parse_db(Some("db3")).unwrap(), 3);
        assert_eq!(parse_db(Some("12")).unwrap(), 12);
        assert!(parse_db(Some("main")).is_err());
        assert!(parse_db(Some("5000")).is_err());
    }

    #[test]
    fn keyspace_count_covers_the_highest_database_with_keys() {
        let info =
            "# Keyspace\r\ndb0:keys=1,expires=0,avg_ttl=0\r\ndb3:keys=2,expires=1,avg_ttl=9\r\n";
        assert_eq!(keyspace_database_count(info), 4);
        assert_eq!(keyspace_database_count("# Keyspace\r\n"), 0);
        // A server that ignores the section argument returns the whole INFO; unrelated lines are
        // ignored rather than misparsed.
        assert_eq!(
            keyspace_database_count("redis_version:7.0.0\r\ndb1:keys=1\r\n"),
            2
        );
    }

    #[test]
    fn parses_node_lists() {
        let nodes = parse_nodes(
            Some(&"10.0.0.1:26379, 10.0.0.2:26380 ".to_string()),
            26379,
            26379,
        )
        .unwrap();
        assert_eq!(
            nodes,
            vec![
                ("10.0.0.1".to_string(), 26379),
                ("10.0.0.2".to_string(), 26380)
            ]
        );
        let nodes = parse_nodes(Some(&"sentinel.local".to_string()), 26379, 26379).unwrap();
        assert_eq!(nodes, vec![("sentinel.local".to_string(), 26379)]);
    }

    #[test]
    fn key_limit_defaults_and_clamps() {
        assert_eq!(parse_key_limit(None).unwrap(), DEFAULT_KEY_LIMIT);
        assert_eq!(parse_key_limit(Some(&"500".to_string())).unwrap(), 500);
        assert!(parse_key_limit(Some(&"0".to_string())).is_err());
        assert_eq!(
            parse_key_limit(Some(&"999999".to_string())).unwrap(),
            200_000
        );
    }

    #[test]
    fn splits_stream_replies_into_rows() {
        let value = Value::Array(vec![Value::Array(vec![
            Value::BulkString(b"1-0".to_vec()),
            Value::Array(vec![
                Value::BulkString(b"a".to_vec()),
                Value::BulkString(b"1".to_vec()),
                Value::BulkString(b"b".to_vec()),
                Value::BulkString(b"2".to_vec()),
            ]),
        ])]);
        let entries = stream_entries(&value);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "1-0");
        assert_eq!(entries[0].1.len(), 2);
    }
}
