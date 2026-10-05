//! Runtime-loaded ODBC driver-manager binding.
//!
//! We deliberately do **not** link against `libodbc`/`odbc32` (which `odbc-sys`/`odbc-api` do via a
//! `#[link]` attribute). Instead the driver manager is `dlopen`ed/`LoadLibrary`ed on first use and
//! the handful of ODBC C entry points we need are resolved by name. This keeps the binary free of
//! any link-time or hard runtime dependency on ODBC: without a driver manager the app still runs and
//! [`api`] reports the engine as unavailable.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use rustgrid_core::{CellValue, ColumnInfo};

pub(crate) type Handle = *mut c_void;
pub(crate) type HEnv = Handle;
pub(crate) type Hdbc = Handle;
pub(crate) type Hstmt = Handle;
pub(crate) type SqlReturn = i16;

const SQL_SUCCESS: SqlReturn = 0;
const SQL_SUCCESS_WITH_INFO: SqlReturn = 1;
const SQL_NO_DATA: SqlReturn = 100;

pub(crate) const SQL_HANDLE_ENV: i16 = 1;
pub(crate) const SQL_HANDLE_DBC: i16 = 2;
pub(crate) const SQL_HANDLE_STMT: i16 = 3;
const SQL_ATTR_ODBC_VERSION: i32 = 200;
const SQL_OV_ODBC3: usize = 3;
const SQL_DRIVER_NOPROMPT: u16 = 0;
const SQL_FETCH_NEXT: i16 = 1;
const SQL_FETCH_FIRST: i16 = 2;
const SQL_DBMS_NAME: u16 = 17;
const SQL_NTS: i16 = -3;
pub(crate) const SQL_NULL_DATA: isize = -1;
const SQL_NO_TOTAL: isize = -4;
const SQL_C_WCHAR: i16 = -8;
const SQL_PARAM_INPUT: i16 = 1;
const SQL_WVARCHAR: i16 = -9;

fn is_success(ret: SqlReturn) -> bool {
    ret == SQL_SUCCESS || ret == SQL_SUCCESS_WITH_INFO
}

// ----- Raw entry-point signatures (ODBC C ABI) ------------------------------------------------

type SqlAllocHandleFn =
    unsafe extern "system" fn(handle_type: i16, input: Handle, output: *mut Handle) -> SqlReturn;
type SqlFreeHandleFn = unsafe extern "system" fn(handle_type: i16, handle: Handle) -> SqlReturn;
type SqlSetEnvAttrFn = unsafe extern "system" fn(
    env: HEnv,
    attribute: i32,
    value: *mut c_void,
    string_length: i32,
) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlDriverConnectWFn = unsafe extern "system" fn(
    dbc: Hdbc,
    window: Handle,
    in_string: *const u16,
    in_length: i16,
    out_string: *mut u16,
    out_capacity: i16,
    out_length: *mut i16,
    completion: u16,
) -> SqlReturn;
type SqlDisconnectFn = unsafe extern "system" fn(dbc: Hdbc) -> SqlReturn;
type SqlExecDirectWFn =
    unsafe extern "system" fn(stmt: Hstmt, sql: *const u16, sql_length: i32) -> SqlReturn;
type SqlNumResultColsFn = unsafe extern "system" fn(stmt: Hstmt, count: *mut i16) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlDescribeColWFn = unsafe extern "system" fn(
    stmt: Hstmt,
    column: u16,
    name: *mut u16,
    name_capacity: i16,
    name_length: *mut i16,
    data_type: *mut i16,
    column_size: *mut usize,
    decimal_digits: *mut i16,
    nullable: *mut i16,
) -> SqlReturn;
type SqlFetchFn = unsafe extern "system" fn(stmt: Hstmt) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlGetDataWFn = unsafe extern "system" fn(
    stmt: Hstmt,
    column: u16,
    target_type: i16,
    target: *mut c_void,
    target_capacity: isize,
    indicator: *mut isize,
) -> SqlReturn;
type SqlRowCountFn = unsafe extern "system" fn(stmt: Hstmt, count: *mut isize) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlGetDiagRecWFn = unsafe extern "system" fn(
    handle_type: i16,
    handle: Handle,
    record: i16,
    sql_state: *mut u16,
    native_error: *mut i32,
    message: *mut u16,
    message_capacity: i16,
    message_length: *mut i16,
) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlDriversWFn = unsafe extern "system" fn(
    env: HEnv,
    direction: i16,
    description: *mut u16,
    description_capacity: i16,
    description_length: *mut i16,
    attributes: *mut u16,
    attributes_capacity: i16,
    attributes_length: *mut i16,
) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlGetInfoWFn = unsafe extern "system" fn(
    dbc: Hdbc,
    info_type: u16,
    info: *mut c_void,
    info_capacity: i16,
    info_length: *mut i16,
) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlTablesWFn = unsafe extern "system" fn(
    stmt: Hstmt,
    catalog: *const u16,
    catalog_length: i16,
    schema: *const u16,
    schema_length: i16,
    table: *const u16,
    table_length: i16,
    table_type: *const u16,
    table_type_length: i16,
) -> SqlReturn;
type SqlPrepareWFn =
    unsafe extern "system" fn(stmt: Hstmt, sql: *const u16, sql_length: i32) -> SqlReturn;
#[allow(clippy::type_complexity)]
type SqlBindParameterFn = unsafe extern "system" fn(
    stmt: Hstmt,
    parameter: u16,
    input_output_type: i16,
    value_type: i16,
    parameter_type: i16,
    column_size: usize,
    decimal_digits: i16,
    value: *mut c_void,
    buffer_length: isize,
    indicator: *mut isize,
) -> SqlReturn;
type SqlExecuteFn = unsafe extern "system" fn(stmt: Hstmt) -> SqlReturn;

/// The loaded ODBC driver manager and its resolved entry points.
pub(crate) struct OdbcApi {
    _library: libloading::Library,
    pub(crate) environment: HEnv,
    alloc_handle: SqlAllocHandleFn,
    free_handle: SqlFreeHandleFn,
    driver_connect: SqlDriverConnectWFn,
    disconnect: SqlDisconnectFn,
    exec_direct: SqlExecDirectWFn,
    num_result_cols: SqlNumResultColsFn,
    describe_col: SqlDescribeColWFn,
    fetch: SqlFetchFn,
    get_data: SqlGetDataWFn,
    row_count: SqlRowCountFn,
    get_diag_rec: SqlGetDiagRecWFn,
    drivers: SqlDriversWFn,
    get_info: SqlGetInfoWFn,
    tables: SqlTablesWFn,
    prepare: SqlPrepareWFn,
    bind_parameter: SqlBindParameterFn,
    execute_statement: SqlExecuteFn,
}

// The handles are only ever used behind a mutex, and the function pointers are plain addresses.
unsafe impl Send for OdbcApi {}
unsafe impl Sync for OdbcApi {}

static API: OnceLock<Result<OdbcApi, String>> = OnceLock::new();

/// The process-wide ODBC driver manager, loaded on first use. Returns a human-readable error when
/// no driver manager is installed (or a required entry point is missing).
pub(crate) fn api() -> Result<&'static OdbcApi, String> {
    match API.get_or_init(load) {
        Ok(api) => Ok(api),
        Err(error) => Err(error.clone()),
    }
}

fn candidate_libraries() -> &'static [&'static str] {
    #[cfg(target_os = "windows")]
    {
        &["odbc32.dll"]
    }
    #[cfg(target_os = "macos")]
    {
        &[
            "libodbc.2.dylib",
            "libodbc.dylib",
            "libiodbc.2.dylib",
            "libiodbc.dylib",
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &["libodbc.so.2", "libodbc.so"]
    }
    #[cfg(not(any(target_os = "windows", unix)))]
    {
        &[]
    }
}

fn load() -> Result<OdbcApi, String> {
    let mut library = None;
    for name in candidate_libraries() {
        // SAFETY: loading a system library and resolving well-known symbols from it.
        if let Ok(loaded) = unsafe { libloading::Library::new(*name) } {
            library = Some(loaded);
            break;
        }
    }
    let library = library.ok_or_else(|| {
        "no ODBC driver manager found (install unixODBC/iODBC on Linux/macOS)".to_string()
    })?;

    // SAFETY: each symbol is resolved by its documented C name and cast to a matching signature.
    unsafe {
        let alloc_handle: SqlAllocHandleFn = resolve(&library, b"SQLAllocHandle\0")?;
        let free_handle: SqlFreeHandleFn = resolve(&library, b"SQLFreeHandle\0")?;
        let set_env_attr: SqlSetEnvAttrFn = resolve(&library, b"SQLSetEnvAttr\0")?;
        let driver_connect: SqlDriverConnectWFn = resolve(&library, b"SQLDriverConnectW\0")?;
        let disconnect: SqlDisconnectFn = resolve(&library, b"SQLDisconnect\0")?;
        let exec_direct: SqlExecDirectWFn = resolve(&library, b"SQLExecDirectW\0")?;
        let num_result_cols: SqlNumResultColsFn = resolve(&library, b"SQLNumResultCols\0")?;
        let describe_col: SqlDescribeColWFn = resolve(&library, b"SQLDescribeColW\0")?;
        let fetch: SqlFetchFn = resolve(&library, b"SQLFetch\0")?;
        let get_data: SqlGetDataWFn = resolve(&library, b"SQLGetData\0")?;
        let row_count: SqlRowCountFn = resolve(&library, b"SQLRowCount\0")?;
        let get_diag_rec: SqlGetDiagRecWFn = resolve(&library, b"SQLGetDiagRecW\0")?;
        let drivers: SqlDriversWFn = resolve(&library, b"SQLDriversW\0")?;
        let get_info: SqlGetInfoWFn = resolve(&library, b"SQLGetInfoW\0")?;
        let tables: SqlTablesWFn = resolve(&library, b"SQLTablesW\0")?;
        let prepare: SqlPrepareWFn = resolve(&library, b"SQLPrepareW\0")?;
        let bind_parameter: SqlBindParameterFn = resolve(&library, b"SQLBindParameter\0")?;
        let execute_statement: SqlExecuteFn = resolve(&library, b"SQLExecute\0")?;

        let mut environment: HEnv = null_mut();
        if !is_success(alloc_handle(SQL_HANDLE_ENV, null_mut(), &mut environment)) {
            return Err("failed to allocate the ODBC environment".to_string());
        }
        let version = SQL_OV_ODBC3 as *mut c_void;
        if !is_success(set_env_attr(environment, SQL_ATTR_ODBC_VERSION, version, 0)) {
            let _ = free_handle(SQL_HANDLE_ENV, environment);
            return Err("failed to declare ODBC version 3".to_string());
        }

        Ok(OdbcApi {
            _library: library,
            environment,
            alloc_handle,
            free_handle,
            driver_connect,
            disconnect,
            exec_direct,
            num_result_cols,
            describe_col,
            fetch,
            get_data,
            row_count,
            get_diag_rec,
            drivers,
            get_info,
            tables,
            prepare,
            bind_parameter,
            execute_statement,
        })
    }
}

unsafe fn resolve<T: Copy>(library: &libloading::Library, name: &[u8]) -> Result<T, String> {
    // SAFETY: the caller guarantees `name` is a valid, documented ODBC symbol.
    let symbol = unsafe { library.get::<T>(name) }
        .map_err(|error| format!("missing ODBC entry point {}: {error}", symbol_name(name)))?;
    Ok(*symbol)
}

fn symbol_name(name: &[u8]) -> String {
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    String::from_utf8_lossy(&name[..end]).into_owned()
}

impl OdbcApi {
    /// The most recent diagnostic message on `handle`, or a generic fallback.
    pub(crate) fn error_message(&self, handle_type: i16, handle: Handle) -> String {
        let mut message = String::new();
        let mut record = 1;
        loop {
            let mut state = [0u16; 6];
            let mut native_error: i32 = 0;
            let mut text = vec![0u16; 1024];
            // SAFETY: `handle` is a live ODBC handle of `handle_type`.
            let ret = unsafe {
                (self.get_diag_rec)(
                    handle_type,
                    handle,
                    record,
                    state.as_mut_ptr(),
                    &mut native_error,
                    text.as_mut_ptr(),
                    text.len() as i16,
                    null_mut(),
                )
            };
            if !is_success(ret) {
                break;
            }
            if !message.is_empty() {
                message.push_str("; ");
            }
            message.push_str(&from_wide(&text));
            record += 1;
        }
        if message.is_empty() {
            "ODBC call failed".to_string()
        } else {
            message
        }
    }

    /// The installed driver descriptions, for the connection form's driver dropdown.
    pub(crate) fn driver_descriptions(&self) -> Result<Vec<String>, String> {
        let mut drivers = Vec::new();
        let mut description = vec![0u16; 1024];
        let mut attributes = vec![0u16; 1024];
        let mut direction = SQL_FETCH_FIRST;
        loop {
            let mut description_length: i16 = 0;
            let mut attributes_length: i16 = 0;
            // SAFETY: the environment is live and the buffers are valid for their lengths.
            let ret = unsafe {
                (self.drivers)(
                    self.environment,
                    direction,
                    description.as_mut_ptr(),
                    description.len() as i16,
                    &mut description_length,
                    attributes.as_mut_ptr(),
                    attributes.len() as i16,
                    &mut attributes_length,
                )
            };
            if ret == SQL_NO_DATA {
                break;
            }
            if !is_success(ret) {
                return Err(self.error_message(SQL_HANDLE_ENV, self.environment));
            }
            let name = from_wide(&description);
            if !name.is_empty() {
                drivers.push(name);
            }
            direction = SQL_FETCH_NEXT;
        }
        drivers.sort();
        drivers.dedup();
        Ok(drivers)
    }

    /// Open a connection with `connection_string`, returning the connection handle.
    pub(crate) fn connect(&self, connection_string: &str) -> Result<Hdbc, String> {
        let mut dbc: Hdbc = null_mut();
        // SAFETY: allocating a connection handle from the live environment.
        if !is_success(unsafe { (self.alloc_handle)(SQL_HANDLE_DBC, self.environment, &mut dbc) }) {
            return Err("failed to allocate the ODBC connection".to_string());
        }

        let input = to_wide(connection_string);
        let mut output = vec![0u16; 1024];
        let mut output_length: i16 = 0;
        // SAFETY: `dbc` is live and the buffers are valid for their lengths.
        let ret = unsafe {
            (self.driver_connect)(
                dbc,
                null_mut(),
                input.as_ptr(),
                SQL_NTS,
                output.as_mut_ptr(),
                output.len() as i16,
                &mut output_length,
                SQL_DRIVER_NOPROMPT,
            )
        };
        if !is_success(ret) {
            let message = self.error_message(SQL_HANDLE_DBC, dbc);
            unsafe { (self.free_handle)(SQL_HANDLE_DBC, dbc) };
            return Err(message);
        }
        Ok(dbc)
    }

    pub(crate) fn disconnect(&self, dbc: Hdbc) {
        // SAFETY: `dbc` is a live connection handle.
        let _ = unsafe { (self.disconnect)(dbc) };
    }

    pub(crate) fn free_connection(&self, dbc: Hdbc) {
        // SAFETY: `dbc` is a live connection handle.
        let _ = unsafe { (self.free_handle)(SQL_HANDLE_DBC, dbc) };
    }

    /// Allocate a statement on `dbc`.
    pub(crate) fn alloc_statement(&self, dbc: Hdbc) -> Result<Hstmt, String> {
        let mut stmt: Hstmt = null_mut();
        // SAFETY: allocating a statement handle from the live connection.
        if !is_success(unsafe { (self.alloc_handle)(SQL_HANDLE_STMT, dbc, &mut stmt) }) {
            return Err(self.error_message(SQL_HANDLE_DBC, dbc));
        }
        Ok(stmt)
    }

    pub(crate) fn free_statement(&self, stmt: Hstmt) {
        // SAFETY: `stmt` is a live statement handle.
        let _ = unsafe { (self.free_handle)(SQL_HANDLE_STMT, stmt) };
    }

    /// Run `sql`, returning the raw statement handle (still owned by the caller).
    pub(crate) fn execute(&self, dbc: Hdbc, sql: &str) -> Result<Hstmt, String> {
        let stmt = self.alloc_statement(dbc)?;
        let sql = to_wide(sql);
        // SAFETY: `stmt` is live and the NUL-terminated SQL buffer outlives the call.
        let ret = unsafe { (self.exec_direct)(stmt, sql.as_ptr(), SQL_NTS as i32) };
        if !is_success(ret) {
            let message = self.error_message(SQL_HANDLE_STMT, stmt);
            self.free_statement(stmt);
            return Err(message);
        }
        Ok(stmt)
    }

    /// Prepare `sql` with `?` placeholders, returning the statement for binding.
    pub(crate) fn prepare(&self, dbc: Hdbc, sql: &str) -> Result<Hstmt, String> {
        let stmt = self.alloc_statement(dbc)?;
        let sql = to_wide(sql);
        // SAFETY: `stmt` is live and the NUL-terminated SQL buffer outlives the call.
        let ret = unsafe { (self.prepare)(stmt, sql.as_ptr(), SQL_NTS as i32) };
        if !is_success(ret) {
            let message = self.error_message(SQL_HANDLE_STMT, stmt);
            self.free_statement(stmt);
            return Err(message);
        }
        Ok(stmt)
    }

    /// Bind `params` as text (SQL NULL for `None`) and execute the prepared statement.
    pub(crate) fn execute_bound(
        &self,
        stmt: Hstmt,
        params: &[Option<String>],
    ) -> Result<(), String> {
        let mut buffers: Vec<Vec<u16>> = Vec::with_capacity(params.len());
        let mut indicators: Vec<isize> = Vec::with_capacity(params.len());
        for param in params {
            let mut buffer = match param {
                Some(value) => to_wide(value),
                None => vec![0u16],
            };
            if buffer.is_empty() {
                buffer.push(0);
            }
            indicators.push(match param {
                Some(value) => (value.encode_utf16().count() * 2) as isize,
                None => SQL_NULL_DATA,
            });
            buffers.push(buffer);
        }

        for (index, buffer) in buffers.iter_mut().enumerate() {
            let buffer_length = (buffer.len() * 2) as isize;
            let indicator = &mut indicators[index] as *mut isize;
            // SAFETY: `stmt` is live; the buffer and indicator outlive the execute call below.
            let ret = unsafe {
                (self.bind_parameter)(
                    stmt,
                    (index + 1) as u16,
                    SQL_PARAM_INPUT,
                    SQL_C_WCHAR,
                    SQL_WVARCHAR,
                    buffer.len(),
                    0,
                    buffer.as_mut_ptr() as *mut c_void,
                    buffer_length,
                    indicator,
                )
            };
            if !is_success(ret) {
                return Err(self.error_message(SQL_HANDLE_STMT, stmt));
            }
        }

        // SAFETY: `stmt` is live and every bound buffer is still alive here.
        let ret = unsafe { (self.execute_statement)(stmt) };
        if !is_success(ret) {
            return Err(self.error_message(SQL_HANDLE_STMT, stmt));
        }
        Ok(())
    }

    /// The DBMS name reported by the driver, if any.
    pub(crate) fn dbms_name(&self, dbc: Hdbc) -> Option<String> {
        let mut buffer = vec![0u16; 256];
        let mut length: i16 = 0;
        // SAFETY: `dbc` is live and the buffer is valid for its length.
        let ret = unsafe {
            (self.get_info)(
                dbc,
                SQL_DBMS_NAME,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as i16,
                &mut length,
            )
        };
        if is_success(ret) {
            let name = from_wide(&buffer);
            return (!name.is_empty()).then_some(name);
        }
        None
    }

    /// Read the metadata and text values of the result set over `stmt`.
    pub(crate) fn read_result(
        &self,
        stmt: Hstmt,
    ) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>), String> {
        let mut count: i16 = 0;
        // SAFETY: `stmt` is live.
        if !is_success(unsafe { (self.num_result_cols)(stmt, &mut count) }) {
            return Err(self.error_message(SQL_HANDLE_STMT, stmt));
        }
        if count <= 0 {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut columns = Vec::with_capacity(count as usize);
        for index in 1..=count as u16 {
            let mut name = vec![0u16; 256];
            let mut name_length: i16 = 0;
            let mut data_type: i16 = 0;
            let mut column_size: usize = 0;
            let mut decimal_digits: i16 = 0;
            let mut nullable: i16 = 0;
            // SAFETY: `stmt` is live and the out-pointers are valid.
            let ret = unsafe {
                (self.describe_col)(
                    stmt,
                    index,
                    name.as_mut_ptr(),
                    name.len() as i16,
                    &mut name_length,
                    &mut data_type,
                    &mut column_size,
                    &mut decimal_digits,
                    &mut nullable,
                )
            };
            let name = if is_success(ret) {
                from_wide(&name)
            } else {
                String::new()
            };
            columns.push(ColumnInfo {
                name,
                data_type: type_name(data_type).to_string(),
                nullable: nullable != 0,
                primary_key: false,
                comment: String::new(),
            });
        }

        let mut rows = Vec::new();
        loop {
            // SAFETY: `stmt` is live.
            let fetch = unsafe { (self.fetch)(stmt) };
            if fetch == SQL_NO_DATA {
                break;
            }
            if !is_success(fetch) {
                return Err(self.error_message(SQL_HANDLE_STMT, stmt));
            }
            let mut values = Vec::with_capacity(count as usize);
            for index in 1..=count as u16 {
                values.push(self.read_column(stmt, index)?);
            }
            rows.push(values);
        }
        Ok((columns, rows))
    }

    fn read_column(&self, stmt: Hstmt, index: u16) -> Result<CellValue, String> {
        let mut units: Vec<u16> = Vec::new();
        let mut chunk = vec![0u16; 1024];
        loop {
            let mut indicator: isize = 0;
            // SAFETY: `stmt` is live and the chunk is valid for its byte length.
            let ret = unsafe {
                (self.get_data)(
                    stmt,
                    index,
                    SQL_C_WCHAR,
                    chunk.as_mut_ptr() as *mut c_void,
                    (chunk.len() * 2) as isize,
                    &mut indicator,
                )
            };
            if ret == SQL_NO_DATA {
                break;
            }
            if !is_success(ret) {
                return Err(self.error_message(SQL_HANDLE_STMT, stmt));
            }
            if indicator == SQL_NULL_DATA {
                return Ok(CellValue::Null);
            }
            let taken = if indicator == SQL_NO_TOTAL {
                chunk
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(chunk.len())
            } else {
                ((indicator as usize) / 2).min(chunk.len().saturating_sub(1))
            };
            units.extend_from_slice(&chunk[..taken]);
            if ret == SQL_SUCCESS {
                break;
            }
        }
        Ok(CellValue::Text(String::from_utf16_lossy(&units)))
    }

    /// The rows affected by the last statement, if the driver reports them.
    pub(crate) fn rows_affected(&self, stmt: Hstmt) -> Option<u64> {
        let mut count: isize = 0;
        // SAFETY: `stmt` is live.
        if is_success(unsafe { (self.row_count)(stmt, &mut count) }) && count >= 0 {
            Some(count as u64)
        } else {
            None
        }
    }

    /// Run a `SQLTables` catalog query over `dbc` and return its result set.
    pub(crate) fn tables(
        &self,
        dbc: Hdbc,
        catalog: &str,
        schema: &str,
        table: &str,
        table_type: &str,
    ) -> Result<(Vec<ColumnInfo>, Vec<Vec<CellValue>>), String> {
        let stmt = self.alloc_statement(dbc)?;
        let catalog = optional_wide(catalog);
        let schema = optional_wide(schema);
        let table = optional_wide(table);
        let table_type = optional_wide(table_type);
        let length = |value: &Option<Vec<u16>>| if value.is_some() { SQL_NTS } else { 0 };
        // SAFETY: `stmt` is live; each filter is either a NUL-terminated buffer or a NULL pointer
        // (which ODBC reads as "no filter for this field").
        let ret = unsafe {
            (self.tables)(
                stmt,
                catalog.as_ref().map_or(null(), |value| value.as_ptr()),
                length(&catalog),
                schema.as_ref().map_or(null(), |value| value.as_ptr()),
                length(&schema),
                table.as_ref().map_or(null(), |value| value.as_ptr()),
                length(&table),
                table_type.as_ref().map_or(null(), |value| value.as_ptr()),
                length(&table_type),
            )
        };
        if !is_success(ret) {
            let message = self.error_message(SQL_HANDLE_STMT, stmt);
            self.free_statement(stmt);
            return Err(message);
        }
        let result = self.read_result(stmt);
        self.free_statement(stmt);
        result
    }
}

/// Encode `value` as NUL-terminated UTF-16.
fn to_wide(value: &str) -> Vec<u16> {
    let mut units: Vec<u16> = value.encode_utf16().collect();
    units.push(0);
    units
}

/// Encode a non-empty filter as NUL-terminated UTF-16; `None` means "no filter" (a NULL pointer).
fn optional_wide(value: &str) -> Option<Vec<u16>> {
    (!value.is_empty()).then(|| to_wide(value))
}

/// Decode a NUL-terminated (or fixed-width) UTF-16 buffer, stopping at the first NUL.
fn from_wide(buffer: &[u16]) -> String {
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

/// A readable name for the ODBC SQL data type code.
fn type_name(code: i16) -> &'static str {
    match code {
        -7 => "bit",
        -6 => "tinyint",
        -5 => "bigint",
        -4 => "longvarbinary",
        -3 => "varbinary",
        -2 => "binary",
        -1 => "longvarchar",
        -8 => "wchar",
        -9 => "wvarchar",
        -10 => "wlongvarchar",
        -11 => "guid",
        1 => "char",
        2 => "numeric",
        3 => "decimal",
        4 => "integer",
        5 => "smallint",
        6 => "float",
        7 => "real",
        8 => "double",
        12 => "varchar",
        91 => "date",
        92 => "time",
        93 => "timestamp",
        _ => "unknown",
    }
}
