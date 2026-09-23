//! Live MySQL stored-routine lifecycle test.
//!
//! It reuses the connection saved by the RustGrid app (reading the encrypted password through
//! `ConfigStore`), so no plaintext credential is needed. When no profile is saved it falls back
//! to the `RUSTGRID_MYSQL_*` environment variables, matching `rustgrid-mysql`'s live test.
//!
//! Run with:
//!   cargo test -p rustgrid-app --test live_routine -- --ignored --nocapture
//!
//! The test creates and drops its own scratch database (`rustgrid_routine_test`).

use std::collections::BTreeMap;

use rustgrid_core::{ConnectionConfig, Driver, DriverId, RoutineEdit, RoutineKind};
use rustgrid_mysql::MysqlDriver;

fn config() -> ConnectionConfig {
    if let Ok(password) = std::env::var("RUSTGRID_MYSQL_PASSWORD") {
        return ConnectionConfig {
            driver: DriverId::new("mysql"),
            host: std::env::var("RUSTGRID_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: std::env::var("RUSTGRID_MYSQL_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3306),
            username: std::env::var("RUSTGRID_MYSQL_USER").unwrap_or_else(|_| "root".to_string()),
            password: Some(password),
            database: None,
            options: BTreeMap::new(),
        };
    }

    let store = rustgrid_config::ConfigStore::new().expect("config dir");
    let profiles = store.load_profiles().expect("saved profiles");
    let secrets = store.load_secrets().unwrap_or_default();
    let profile = profiles
        .first()
        .expect("a saved RustGrid connection (or set RUSTGRID_MYSQL_PASSWORD)");
    ConnectionConfig {
        driver: profile.driver.clone(),
        host: profile.host.clone(),
        port: profile.port,
        username: profile.username.clone(),
        password: secrets.get(&profile.id).cloned(),
        database: None,
        options: BTreeMap::new(),
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live MySQL server (uses the saved RustGrid connection)"]
async fn live_routine_lifecycle() {
    let driver = MysqlDriver::new();
    let connection = driver
        .connect(&config())
        .await
        .expect("connect to the live server");

    let db = "rustgrid_routine_test";

    // A function body is only accepted with binary logging on when it declares its data access.
    let _ = connection
        .execute_query(None, "SET GLOBAL log_bin_trust_function_creators = 1")
        .await;

    connection
        .execute_query(None, &format!("DROP DATABASE IF EXISTS `{db}`"))
        .await
        .expect("drop any previous scratch database");
    connection
        .execute_query(None, &format!("CREATE DATABASE `{db}`"))
        .await
        .expect("create scratch database");

    // 1. A fresh database has no routines.
    let routines = connection
        .list_routine_infos(db)
        .await
        .expect("list routines");
    assert!(
        routines.is_empty(),
        "expected no routines, got {routines:?}"
    );

    // 2. Create a function and a procedure.
    let function_sql = "CREATE FUNCTION `test_add`(a int, b int)\nRETURNS int\nDETERMINISTIC\nBEGIN\n    RETURN a + b;\nEND".to_string();
    connection
        .save_routine(
            db,
            None,
            &RoutineEdit {
                kind: RoutineKind::Function,
                name: "test_add".to_string(),
                definition: function_sql,
            },
        )
        .await
        .expect("save function");

    let procedure_sql =
        "CREATE PROCEDURE `test_proc`()\nBEGIN\n    SELECT 42 AS answer;\nEND".to_string();
    connection
        .save_routine(
            db,
            None,
            &RoutineEdit {
                kind: RoutineKind::Procedure,
                name: "test_proc".to_string(),
                definition: procedure_sql,
            },
        )
        .await
        .expect("save procedure");

    // 3. The list reports both, with the right kinds and metadata.
    let mut routines = connection
        .list_routine_infos(db)
        .await
        .expect("list routines");
    routines.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(routines.len(), 2, "got {routines:?}");
    assert_eq!(routines[0].name, "test_add");
    assert_eq!(routines[0].kind, RoutineKind::Function);
    assert_eq!(routines[0].return_type, "int");
    assert_eq!(routines[1].name, "test_proc");
    assert_eq!(routines[1].kind, RoutineKind::Procedure);

    // 4. `routine_details` returns the full `SHOW CREATE` statement and settings.
    let details = connection
        .routine_details(db, RoutineKind::Function, "test_add")
        .await
        .expect("function details");
    assert!(
        details.definition.contains("test_add"),
        "definition: {}",
        details.definition
    );
    assert!(
        details.definition.to_uppercase().contains("DETERMINISTIC"),
        "definition: {}",
        details.definition
    );
    assert_eq!(details.info.kind, RoutineKind::Function);
    assert!(!details.character_set_client.is_empty());

    // 5. The function actually runs.
    let result = connection
        .execute_query(
            Some(db),
            &format!("SELECT `{db}`.`test_add`(2, 3) AS total"),
        )
        .await
        .expect("run function");
    assert!(result.has_result_set);
    assert_eq!(
        result
            .rows
            .first()
            .and_then(|row| row.first())
            .map(|cell| cell.as_display()),
        Some("5".to_string())
    );

    // 6. Updating (rename + new body) drops the original and creates the new one.
    let updated_sql = "CREATE FUNCTION `test_add2`(a int, b int)\nRETURNS int\nDETERMINISTIC\nBEGIN\n    RETURN a + b + 1;\nEND".to_string();
    connection
        .save_routine(
            db,
            Some(("test_add", RoutineKind::Function)),
            &RoutineEdit {
                kind: RoutineKind::Function,
                name: "test_add2".to_string(),
                definition: updated_sql.clone(),
            },
        )
        .await
        .expect("update function");
    let routines = connection
        .list_routine_infos(db)
        .await
        .expect("list routines");
    assert!(routines.iter().any(|routine| routine.name == "test_add2"));
    assert!(!routines.iter().any(|routine| routine.name == "test_add"));

    // 7. The SQL preview drops the original before creating the edit.
    let preview = connection.routine_sql(
        db,
        Some(("test_add2", RoutineKind::Function)),
        &RoutineEdit {
            kind: RoutineKind::Function,
            name: "test_add2".to_string(),
            definition: updated_sql,
        },
    );
    assert!(
        preview.contains("DROP FUNCTION IF EXISTS"),
        "preview: {preview}"
    );
    assert!(preview.contains("CREATE FUNCTION"), "preview: {preview}");

    // 8. Dropping both leaves the database empty again.
    connection
        .drop_routine(db, RoutineKind::Function, "test_add2")
        .await
        .expect("drop function");
    connection
        .drop_routine(db, RoutineKind::Procedure, "test_proc")
        .await
        .expect("drop procedure");
    let routines = connection
        .list_routine_infos(db)
        .await
        .expect("list routines");
    assert!(
        routines.is_empty(),
        "expected no routines, got {routines:?}"
    );

    // 9. Clean up.
    connection
        .execute_query(None, &format!("DROP DATABASE `{db}`"))
        .await
        .expect("drop scratch database");
    connection.close().await.expect("close");
}
