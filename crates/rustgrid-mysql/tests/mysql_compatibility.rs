//! Exercise SQLx's real login path against a small MySQL wire-protocol fixture. This verifies the
//! commands sent before `after_connect` (it is not a StarRocks emulator) and the text-protocol
//! fallback for MySQL-protocol proxies that reject server-side prepares.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use rustgrid_core::{
    ConnectionConfig, Driver, DriverId, FilterCondition, FilterNode, FilterOperator, ObjectKind,
    PageRequest, RowInsert, RowUpdate, TlsMode,
};
use rustgrid_mysql::{MariaDbDriver, MysqlDriver};

fn read_packet(stream: &mut TcpStream) -> Vec<u8> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).expect("packet header");
    let len = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).expect("packet payload");
    payload
}

fn write_packet(stream: &mut TcpStream, sequence: u8, payload: &[u8]) {
    let len = (payload.len() as u32).to_le_bytes();
    stream
        .write_all(&[len[0], len[1], len[2], sequence])
        .unwrap();
    stream.write_all(payload).unwrap();
}

const OK: &[u8] = &[0, 0, 0, 2, 0, 0, 0];
const EOF: &[u8] = &[0xfe, 0, 0, 2, 0];

fn serve(listener: TcpListener, reject_sql_mode: bool) -> Vec<String> {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    // Protocol 4.1, secure connection, plugin authentication, multi-statements/results.
    // Deliberately omit TLS and DEPRECATE_EOF to keep the fixture small.
    let capabilities: u32 = 1 | 0x200 | 0x8000 | 0x20000 | 0x40000 | 0x80000;
    let mut greeting = b"\x0a8.0.33\0\x01\0\0\0abcdefgh\0".to_vec();
    greeting.extend_from_slice(&(capabilities as u16).to_le_bytes());
    greeting.push(45);
    greeting.extend_from_slice(&[2, 0]);
    greeting.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes());
    greeting.push(21);
    greeting.extend_from_slice(&[0; 10]);
    greeting.extend_from_slice(b"ijklmnopqrst\0mysql_native_password\0");
    write_packet(&mut stream, 0, &greeting);
    let _login = read_packet(&mut stream);
    write_packet(&mut stream, 2, OK);

    let mut queries = Vec::new();
    loop {
        let packet = read_packet(&mut stream);
        match packet[0] {
            1 => break,                             // COM_QUIT
            14 => write_packet(&mut stream, 1, OK), // COM_PING (pool checkout)
            3 => {
                let sql = String::from_utf8(packet[1..].to_vec()).unwrap();
                if reject_sql_mode {
                    assert!(!sql.contains("sql_mode"), "incompatible login SQL: {sql}");
                }
                if sql == "SHOW DATABASES" {
                    write_packet(&mut stream, 1, &[1]);
                    // One VAR_STRING column, followed by one text row.
                    let mut column = b"\x03def\x00\x00\x00\x08Database\x00\x0c".to_vec();
                    column.extend_from_slice(&[45, 0, 255, 0, 0, 0, 253, 0, 0, 0, 0, 0]);
                    write_packet(&mut stream, 2, &column);
                    write_packet(&mut stream, 3, EOF);
                    write_packet(&mut stream, 4, b"\x12information_schema");
                    write_packet(&mut stream, 5, EOF);
                } else {
                    assert!(sql.starts_with("SET "), "unexpected command: {sql}");
                    write_packet(&mut stream, 1, OK);
                }
                queries.push(sql);
            }
            command => panic!("unexpected protocol command {command}; SHOW must use COM_QUERY"),
        }
    }
    queries
}

async fn commands(driver: &dyn Driver, mode: Option<&str>, reject_sql_mode: bool) -> Vec<String> {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || serve(listener, reject_sql_mode));
    let mut options = BTreeMap::new();
    if let Some(mode) = mode {
        options.insert("mysql.compatibility".to_string(), mode.to_string());
    }
    let mut config = ConnectionConfig {
        driver: DriverId::new(driver.id().as_str()),
        host: "127.0.0.1".to_string(),
        port,
        username: "root".to_string(),
        password: None,
        database: None,
        options,
        settings: Default::default(),
    };
    config.settings.tls.mode = TlsMode::Disabled;
    config.settings.connect_timeout = Some(2);
    // Confirm session initialization still runs after SQLx's own login commands.
    config.settings.init_sql = "SET @rustgrid_probe = 1".to_string();
    let connection = driver.connect(&config).await.expect("connect to fixture");
    let databases = connection.list_databases().await.expect("list databases");
    assert_eq!(databases[0].name, "information_schema");
    connection.close().await.unwrap();
    server.join().expect("wire-protocol fixture")
}

#[tokio::test]
async fn starrocks_skips_sql_mode_but_keeps_other_initialization() {
    let queries = commands(&MysqlDriver::new(), Some("starrocks"), true).await;
    assert!(queries[0].contains("NAMES utf8mb4"));
    assert_eq!(queries[1], "SET @rustgrid_probe = 1");
    assert_eq!(queries[2], "SHOW DATABASES");
}

#[tokio::test]
async fn mysql_defaults_keep_both_sql_modes() {
    for mode in [None, Some("default"), Some("unknown")] {
        let queries = commands(&MysqlDriver::new(), mode, false).await;
        assert!(queries[0].contains("sql_mode=(SELECT CONCAT(@@sql_mode"));
        assert!(queries[0].contains("PIPES_AS_CONCAT,NO_ENGINE_SUBSTITUTION"));
    }
}

#[tokio::test]
async fn mariadb_ignores_mysql_compatibility_option() {
    let queries = commands(&MariaDbDriver::new(), Some("starrocks"), false).await;
    assert!(queries[0].contains("PIPES_AS_CONCAT,NO_ENGINE_SUBSTITUTION"));
    assert!(
        MariaDbDriver::new()
            .descriptor()
            .connection_form
            .options
            .is_empty()
    );
}

// ----- Fallback for a proxy that rejects server-side prepares -------------------------------
//
// UDAL/TeleDB's DBProxy answers `1047 ER_UNKNOWN_COM_ERROR` to `COM_STMT_PREPARE` (the issue
// reported "1047 - UDAL - Unknown command" when a database was opened), while the text protocol
// works — which is how Navicat talks to it. It also refuses every statement — even a fully
// qualified `information_schema` one — with `1046 ER_NO_DB_ERROR` ("No database selected") until
// the session selects a database, the second shape of the same issue. The fixture below emulates
// both: a statement without a current database gets 1046, a prepared statement after a `USE` gets
// 1047, and the classic `SHOW` statements plus the generated text statements are served. The
// connection therefore has to select the database and then fall back for its table list, columns,
// overview, pages and writes.

const FULL_COLUMNS: &[&str] = &[
    "Field",
    "Type",
    "Collation",
    "Null",
    "Key",
    "Default",
    "Extra",
    "Privileges",
    "Comment",
];

fn error_packet(code: u16, state: &str, message: &str) -> Vec<u8> {
    let mut payload = vec![0xff];
    payload.extend_from_slice(&code.to_le_bytes());
    payload.push(b'#');
    payload.extend_from_slice(state.as_bytes());
    payload.extend_from_slice(message.as_bytes());
    payload
}

fn push_lenenc(payload: &mut Vec<u8>, value: &[u8]) {
    assert!(
        value.len() < 251,
        "fixture values stay one-byte length-encoded"
    );
    payload.push(value.len() as u8);
    payload.extend_from_slice(value);
}

fn text_row(values: &[Option<&str>]) -> Vec<u8> {
    let mut payload = Vec::new();
    for value in values {
        match value {
            Some(text) => push_lenenc(&mut payload, text.as_bytes()),
            None => payload.push(0xfb),
        }
    }
    payload
}

fn column_packet(name: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    push_lenenc(&mut payload, b"def");
    push_lenenc(&mut payload, b"");
    push_lenenc(&mut payload, b"");
    push_lenenc(&mut payload, b"");
    push_lenenc(&mut payload, name.as_bytes());
    push_lenenc(&mut payload, b"");
    payload.push(0x0c);
    payload.extend_from_slice(&45u16.to_le_bytes()); // utf8mb4_general_ci
    payload.extend_from_slice(&1024u32.to_le_bytes());
    payload.push(0xfd); // VAR_STRING
    payload.extend_from_slice(&0u16.to_le_bytes());
    payload.push(0);
    payload.extend_from_slice(&[0, 0]);
    payload
}

fn write_result_set(stream: &mut TcpStream, columns: &[&str], rows: &[Vec<Option<&str>>]) {
    let mut sequence = 1;
    write_packet(stream, sequence, &[columns.len() as u8]);
    sequence += 1;
    for name in columns {
        write_packet(stream, sequence, &column_packet(name));
        sequence += 1;
    }
    write_packet(stream, sequence, EOF);
    sequence += 1;
    for row in rows {
        write_packet(stream, sequence, &text_row(row));
        sequence += 1;
    }
    write_packet(stream, sequence, EOF);
}

fn respond(stream: &mut TcpStream, sql: &str) {
    if sql.starts_with("SHOW FULL TABLES FROM") {
        write_result_set(
            stream,
            &["Tables_in_shop", "Table_type"],
            &[
                vec![Some("users"), Some("BASE TABLE")],
                vec![Some("v"), Some("VIEW")],
            ],
        );
    } else if sql.starts_with("SHOW FULL COLUMNS FROM") {
        write_result_set(
            stream,
            FULL_COLUMNS,
            &[
                vec![
                    Some("id"),
                    Some("int(11)"),
                    None,
                    Some("NO"),
                    Some("PRI"),
                    None,
                    Some("auto_increment"),
                    Some("select,insert,update,references"),
                    Some(""),
                ],
                vec![
                    Some("name"),
                    Some("varchar(64)"),
                    Some("utf8mb4_general_ci"),
                    Some("YES"),
                    Some(""),
                    None,
                    Some(""),
                    Some("select,insert,update,references"),
                    Some("display name"),
                ],
            ],
        );
    } else if sql.starts_with("SHOW TABLE STATUS FROM") {
        write_result_set(
            stream,
            &[
                "Name",
                "Engine",
                "Rows",
                "Auto_increment",
                "Update_time",
                "Data_length",
                "Comment",
            ],
            &[vec![
                Some("users"),
                Some("InnoDB"),
                Some("42"),
                Some("7"),
                None,
                Some("16384"),
                Some("account table"),
            ]],
        );
    } else if sql.starts_with("SELECT routine_name FROM information_schema.routines") {
        write_result_set(stream, &["routine_name"], &[vec![Some("calc_total")]]);
    } else if sql.starts_with("SELECT event_name FROM information_schema.events") {
        write_result_set(stream, &["event_name"], &[vec![Some("nightly_cleanup")]]);
    } else if sql.starts_with("SELECT default_character_set_name, default_collation_name") {
        write_result_set(
            stream,
            &["default_character_set_name", "default_collation_name"],
            &[vec![Some("utf8mb4"), Some("utf8mb4_0900_ai_ci")]],
        );
    } else if sql.starts_with("SET ")
        || sql.starts_with("SELECT ")
        || sql.starts_with("UPDATE ")
        || sql.starts_with("INSERT ")
        || sql.starts_with("DELETE ")
    {
        // Session setup, the best-effort exact row count (an empty result is fine), or one of the
        // text-protocol writes.
        write_packet(stream, 1, OK);
    } else {
        panic!("unexpected command: {sql}");
    }
}

fn read_packet_opt(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).ok()?;
    let len = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).ok()?;
    Some(payload)
}

fn serve_connection(mut stream: TcpStream, queries: std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    // Windows may hand an accepted socket the listener's non-blocking mode down to it.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let capabilities: u32 = 1 | 0x200 | 0x8000 | 0x20000 | 0x40000 | 0x80000;
    let mut greeting = b"\x0a8.0.33-UDAL-DBPROXY\0\x01\0\0\0abcdefgh\0".to_vec();
    greeting.extend_from_slice(&(capabilities as u16).to_le_bytes());
    greeting.push(45);
    greeting.extend_from_slice(&[2, 0]);
    greeting.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes());
    greeting.push(21);
    greeting.extend_from_slice(&[0; 10]);
    greeting.extend_from_slice(b"ijklmnopqrst\0mysql_native_password\0");
    write_packet(&mut stream, 0, &greeting);
    let Some(_login) = read_packet_opt(&mut stream) else {
        return;
    };
    write_packet(&mut stream, 2, OK);

    // UDAL's DBProxy refuses every statement — including a fully qualified
    // `information_schema` query — with `1046 No database selected` while the session has no
    // current database. A `USE` fixes it; prepared statements are then rejected with 1047.
    let mut current_database: Option<String> = None;
    while let Some(packet) = read_packet_opt(&mut stream) {
        match packet[0] {
            1 => break,                             // COM_QUIT
            14 => write_packet(&mut stream, 1, OK), // COM_PING
            3 => {
                let sql = String::from_utf8(packet[1..].to_vec()).unwrap();
                if let Some(database) = sql.strip_prefix("USE ") {
                    current_database = Some(database.trim().trim_matches('`').to_string());
                    write_packet(&mut stream, 1, OK);
                } else if current_database.is_none() && requires_database(&sql) {
                    write_packet(
                        &mut stream,
                        1,
                        &error_packet(
                            1046,
                            "HY000",
                            &format!("UDAL - No database selected : {sql}"),
                        ),
                    );
                } else {
                    respond(&mut stream, &sql);
                }
                queries.lock().unwrap().push(sql);
            }
            0x16 => {
                let sql = String::from_utf8(packet[1..].to_vec()).unwrap();
                let error = if current_database.is_none() {
                    error_packet(
                        1046,
                        "HY000",
                        &format!("UDAL - No database selected : {sql}"),
                    )
                } else {
                    error_packet(1047, "08S01", "UDAL - Unknown command")
                };
                write_packet(&mut stream, 1, &error);
                queries.lock().unwrap().push(format!("PREPARE {sql}"));
            }
            command => panic!("unexpected protocol command {command}"),
        }
    }
}

/// Whether UDAL's DBProxy needs a current database to accept `sql`.
fn requires_database(sql: &str) -> bool {
    sql.starts_with("SHOW FULL")
        || sql.starts_with("SHOW TABLE STATUS")
        || sql.starts_with("SELECT ")
        || sql.starts_with("UPDATE ")
        || sql.starts_with("INSERT ")
        || sql.starts_with("DELETE ")
}

/// A proxy fixture that rejects every prepared statement with `1047 ER_UNKNOWN_COM_ERROR` and
/// serves the text protocol. The pool may open a second connection while the first one is still
/// being returned, so every accepted stream gets its own thread.
struct Proxy {
    port: u16,
    queries: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Proxy {
    fn start() -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{Arc, Mutex};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let accept_queries = Arc::clone(&queries);
        let accept_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut workers = Vec::new();
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let queries = Arc::clone(&accept_queries);
                        workers.push(thread::spawn(move || serve_connection(stream, queries)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        let stopped = accept_stop.load(Ordering::Relaxed);
                        if stopped && workers.iter().all(thread::JoinHandle::is_finished) {
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            port,
            queries,
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) -> Vec<String> {
        use std::sync::atomic::Ordering;

        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
        self.queries.lock().unwrap().clone()
    }
}

async fn proxy_connection(port: u16) -> Box<dyn rustgrid_core::Connection> {
    let mut config = ConnectionConfig {
        driver: DriverId::new("mysql"),
        host: "127.0.0.1".to_string(),
        port,
        username: "root".to_string(),
        password: None,
        database: None,
        options: BTreeMap::new(),
        settings: Default::default(),
    };
    config.settings.tls.mode = TlsMode::Disabled;
    config.settings.connect_timeout = Some(2);
    MysqlDriver::new()
        .connect(&config)
        .await
        .expect("connect to the proxy fixture")
}

#[tokio::test]
async fn catalog_selects_the_database_then_falls_back_after_unknown_command() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    let tables = connection.list_tables("shop").await.expect("list tables");
    assert_eq!(tables.len(), 2);
    assert_eq!(tables[0].name, "users");
    assert_eq!(tables[0].kind, ObjectKind::Table);
    assert_eq!(tables[1].name, "v");
    assert_eq!(tables[1].kind, ObjectKind::View);

    // The session remembered the proxy, so these do not try to prepare again.
    let columns = connection.columns("shop", "users").await.expect("columns");
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0].name, "id");
    assert_eq!(columns[0].data_type, "int(11)");
    assert!(!columns[0].nullable);
    assert!(columns[0].primary_key);
    assert!(columns[0].auto_increment);
    assert_eq!(columns[1].comment, "display name");
    assert!(columns[1].nullable);

    let statuses = connection
        .table_statuses("shop")
        .await
        .expect("table statuses");
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].0, "users");
    assert_eq!(statuses[0].1.engine.as_deref(), Some("InnoDB"));
    assert_eq!(statuses[0].1.rows, Some(42));
    assert_eq!(statuses[0].1.comment.as_deref(), Some("account table"));

    connection.close().await.unwrap();
    let queries = proxy.stop();
    let prepared = queries
        .iter()
        .position(|sql| sql.starts_with("PREPARE SELECT t.table_name, t.table_type"))
        .expect("the information_schema query is tried first");
    // The first prepare answers `1046 No database selected` (UDAL); the driver then selects the
    // database and prepares again, which answers `1047 Unknown command` and lands in the fallback.
    let selected = queries
        .iter()
        .position(|sql| sql == "USE `shop`")
        .expect("the session selects the database after 1046");
    let shown = queries
        .iter()
        .position(|sql| sql == "SHOW FULL TABLES FROM `shop`")
        .expect("SHOW FULL TABLES is the fallback");
    assert!(prepared < selected);
    assert!(selected < shown);
    assert!(
        queries
            .iter()
            .any(|sql| sql == "SHOW FULL COLUMNS FROM `shop`.`users`")
    );
    assert!(
        queries
            .iter()
            .any(|sql| sql == "SHOW TABLE STATUS FROM `shop`")
    );
    assert_eq!(
        queries
            .iter()
            .filter(|sql| sql.starts_with("PREPARE"))
            .count(),
        2,
        "the catalog query is tried once more after the database is selected: {queries:#?}"
    );
}

#[tokio::test]
async fn page_runs_as_one_text_statement_after_the_proxy_fallback() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    // The catalog fallback marks the session, so the page must not try to prepare either.
    connection.list_tables("shop").await.expect("list tables");
    let mut condition = FilterCondition::new("name");
    condition.operator = FilterOperator::Equal;
    condition.value = "o'ne".to_string();
    let page = PageRequest::new(0, 10);
    let page = PageRequest {
        filter: vec![FilterNode::Condition(condition)],
        ..page
    };
    let page = connection
        .fetch_page("shop", "users", page)
        .await
        .expect("page through the text protocol");
    assert!(page.rows.is_empty());
    connection.close().await.unwrap();

    let queries = proxy.stop();
    let statement = queries
        .iter()
        .find(|sql| sql.starts_with("SELECT * FROM `shop`.`users`"))
        .expect("the page is a text statement");
    assert!(
        !statement.contains('?'),
        "bound values are inlined: {statement}"
    );
    assert!(
        statement.contains("`name` = _utf8mb4 0x6f276e65"),
        "{statement}"
    );
    assert!(statement.contains("LIMIT 10 OFFSET 0"), "{statement}");
    assert_eq!(
        queries
            .iter()
            .filter(|sql| sql.starts_with("PREPARE"))
            .count(),
        2,
        "only the catalog probe is prepared (twice, before and after selecting the database): {queries:#?}"
    );
}

#[tokio::test]
async fn backup_object_lists_use_the_text_protocol() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    connection.list_tables("shop").await.expect("list tables");
    let routines = connection.list_routines("shop").await.expect("routines");
    let events = connection.list_events("shop").await.expect("events");
    let options = connection
        .database_options("shop")
        .await
        .expect("database options");
    assert_eq!(routines, ["calc_total"]);
    assert_eq!(events, ["nightly_cleanup"]);
    assert_eq!(options.charset, "utf8mb4");
    assert_eq!(options.collation, "utf8mb4_0900_ai_ci");
    connection.close().await.unwrap();

    let queries = proxy.stop();
    assert!(
        queries.iter().any(|sql| sql
            == "SELECT routine_name FROM information_schema.routines \
                WHERE routine_schema = _utf8mb4 0x73686f70"),
        "{queries:#?}"
    );
    assert!(
        queries.iter().any(|sql| sql
            == "SELECT event_name FROM information_schema.events \
                WHERE event_schema = _utf8mb4 0x73686f70"),
        "{queries:#?}"
    );
    assert_eq!(
        queries
            .iter()
            .filter(|sql| sql.starts_with("PREPARE"))
            .count(),
        2,
        "only the catalog probe is prepared (twice, before and after selecting the database): {queries:#?}"
    );
}

#[tokio::test]
async fn writes_use_the_text_protocol_after_the_proxy_fallback() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    connection.list_tables("shop").await.expect("list tables");
    connection
        .update_rows(
            "shop",
            "users",
            &[RowUpdate {
                set: vec![("name".to_string(), Some("a'b".to_string()))],
                keys: vec![("id".to_string(), Some("1".to_string()))],
            }],
        )
        .await
        .expect("update");
    connection
        .insert_rows(
            "shop",
            "users",
            &[RowInsert {
                values: vec![
                    ("id".to_string(), Some("2".to_string())),
                    ("name".to_string(), None),
                ],
            }],
        )
        .await
        .expect("insert");
    connection
        .delete_rows(
            "shop",
            "users",
            &[vec![("id".to_string(), Some("1".to_string()))]],
        )
        .await
        .expect("delete");
    connection.close().await.unwrap();

    let queries = proxy.stop();
    let update = queries
        .iter()
        .find(|sql| sql.starts_with("UPDATE "))
        .expect("the update is a text statement");
    assert_eq!(
        update, "UPDATE `shop`.`users` SET `name` = 'a\\'b' WHERE `id` = '1'",
        "{queries:#?}"
    );
    assert!(
        queries
            .iter()
            .any(|sql| sql == "INSERT INTO `shop`.`users` (`id`, `name`) VALUES ('2', NULL)"),
        "{queries:#?}"
    );
    assert!(
        queries
            .iter()
            .any(|sql| sql == "DELETE FROM `shop`.`users` WHERE `id` = '1' LIMIT 1"),
        "{queries:#?}"
    );
    assert_eq!(
        queries
            .iter()
            .filter(|sql| sql.starts_with("PREPARE"))
            .count(),
        2,
        "only the catalog probe is prepared (twice, before and after selecting the database): {queries:#?}"
    );
}

#[tokio::test]
async fn columns_fall_back_on_their_own() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    let columns = connection
        .columns("shop", "users")
        .await
        .expect("columns through SHOW FULL COLUMNS");
    assert_eq!(columns[0].name, "id");
    connection.close().await.unwrap();

    let queries = proxy.stop();
    assert!(
        queries
            .iter()
            .any(|sql| sql.starts_with("PREPARE SELECT column_name"))
    );
    assert!(
        queries
            .iter()
            .any(|sql| sql == "SHOW FULL COLUMNS FROM `shop`.`users`")
    );
}

#[tokio::test]
async fn table_statuses_fall_back_on_their_own() {
    let proxy = Proxy::start();
    let connection = proxy_connection(proxy.port).await;

    let statuses = connection
        .table_statuses("shop")
        .await
        .expect("statuses through SHOW TABLE STATUS");
    assert_eq!(statuses[0].1.rows, Some(42));
    connection.close().await.unwrap();

    let queries = proxy.stop();
    assert!(
        queries
            .iter()
            .any(|sql| sql.starts_with("PREPARE SELECT table_name, engine"))
    );
    assert!(
        queries
            .iter()
            .any(|sql| sql == "SHOW TABLE STATUS FROM `shop`")
    );
}
