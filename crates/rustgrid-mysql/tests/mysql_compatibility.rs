//! Exercise SQLx's real login path against a small MySQL wire-protocol fixture.
//! This verifies commands sent before after_connect; it is not a StarRocks emulator.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use rustgrid_core::{ConnectionConfig, Driver, DriverId, TlsMode};
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
    assert!(queries[0].contains("time_zone="));
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
