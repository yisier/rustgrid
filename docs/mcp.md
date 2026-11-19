# Built-in MCP server (stdio)

RustGrid ships a [Model Context Protocol](https://modelcontextprotocol.io) server so a local AI
client can inspect authorized connections and, when explicitly allowed, run read-only SQL. It uses
MCP's **stdio** transport: the client launches the app itself with `--mcp-stdio`, writes one
JSON-RPC object per line to its stdin and reads the responses from stdout. Nothing listens on a
network port and there is no bearer token.

```
RustGrid --mcp-stdio
```

The flag is handled at the very top of `main` — before the single-instance lock, before GPUI and
before any window is created — so it also works while the desktop app is running. stdout is
reserved for protocol messages; diagnostics go to stderr.

## Client setup

The AI / MCP page in Options copies the current executable's absolute path into a ready-to-paste
snippet. The shape is the same for Claude Desktop and Cursor:

```json
{
  "mcpServers": {
    "rustgrid": {
      "command": "C:\\Program Files\\RustGrid\\RustGrid.exe",
      "args": ["--mcp-stdio"]
    }
  }
}
```

On macOS the command is the executable inside the bundle, for example
`/Applications/RustGrid.app/Contents/MacOS/RustGrid`. A generic client that configures one server
directly uses:

```json
{
  "command": "/path/to/RustGrid",
  "args": ["--mcp-stdio"]
}
```

The client owns the process: closing the client closes stdin and the server exits.

## Tools

| Tool | Purpose |
| --- | --- |
| `list_connections` | Authorized connections, engine, access mode, database scope and business context. Sensitive-table names are never disclosed. |
| `get_schema` | Accessible databases, tables/views, columns, keys and comments. |
| `describe_table` | Columns plus the exact `CREATE` DDL (when the engine can produce it). |
| `sample_data` | A few representative rows (default 5, maximum 20). |
| `execute_query` | One SQL statement (Mongo/Redis commands only after an explicit Read-Write grant). |

`execute_query` in **Read-Only** mode accepts exactly one `SELECT`, `WITH … SELECT` or
`EXPLAIN` of one. It parses the statement with `sqlparser` in the connection's
`Driver::dialect_for` dialect and refuses:

- writes, DDL and multiple statements;
- `SELECT … INTO`, `FOR UPDATE`/`FOR SHARE` and other locking reads;
- MySQL/MariaDB executable comments (`/*! … */`, `/*M! … */`) and backslash-quoted string tricks;
- quoted or schema-qualified dangerous functions (`pg_read_file`, `setval`, `LOAD_FILE`,
  `DBMS_*`, …);
- external table functions (`OPENROWSET`, `OPENQUERY`, `file`, `s3`, `url`, `remote`, `dblink`, …);
- relations outside the connection's database scope or on the sensitive-table blacklist.

The result is a compact Markdown table, capped by the configured row limit and a 200 KB payload
budget; a single text value is truncated to 4,096 characters. Key-value and document connections
expose `get_schema`/`sample_data` in read-only mode; their `execute_query` is available only after
the user explicitly grants Read-Write, because a key/command target cannot be audited against the
sensitive-object list.

**This is a guardrail, not a security boundary.** The database account's grants are what actually
protect the data. A read-only database account is the recommended setup; in **Read-Write** mode the
AI can change or delete whatever the account can (the Options UI asks for an explicit confirmation
before enabling it). Read-Write SQL is not parsed: the selected database is checked against the
scope, but a qualified name in arbitrary SQL is bounded only by the account. Blacklisting a base
table does not rewrite a view, stored function or trigger that reads it — blacklist the view too, or
use account grants.

## Permissions (`mcp.json`)

Permissions are machine-local and live in `mcp.json` next to `connections.json`, `settings.json`
and `secrets.json`; they are deliberately not part of a connection profile, so exporting a `.rgc`
does not grant AI access.

```json
{
  "version": 1,
  "enabled": true,
  "max_rows": 50,
  "timeout_seconds": 30,
  "connections": {
    "<profile id>": {
      "enabled": true,
      "read_only": true,
      "allowed_databases": ["shop_dev"],
      "blacklisted_tables": ["passwords", "audit.secrets"],
      "context_prompt": "orders is the source of truth for revenue"
    }
  }
}
```

- `enabled` is the global switch. When it is off the server still speaks the protocol but every
  tool returns a clear "MCP is disabled" error, so the client can stay connected.
- `max_rows` is a hard ceiling (`0` means no cap); a tool may ask for fewer rows, never more.
- `timeout_seconds` bounds each connection/query/metadata operation (clamped to 1–600 seconds).
- `allowed_databases` empty means every database the account can see. For schema-first engines the
  tree's "databases" are schemas (PostgreSQL/SQL Server: schemas inside the connection's database;
  Oracle: schemas); the same names are used in this list.
- `blacklisted_tables` entries match case-insensitively; a bare name matches any schema and a
  qualified `schema.table` entry matches both the dotted name and its last component.
- `context_prompt` is returned by `list_connections` as business instructions.

The file is written atomically by `ConfigStore::save_mcp`, is versioned, and is migrated when a
newer RustGrid reads an older file. A malformed file is reported in Options → AI / MCP rather than
silently replaced by a permissive default.

Tests can redirect the whole configuration directory with `RUSTGRID_CONFIG_DIR`; the app's
integration test uses it with a temporary SQLite database and never touches the user's real
configuration.
