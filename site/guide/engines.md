# Supported engines

RustGrid ships native drivers and one shared UI. Each driver declares what it can do, so actions an engine cannot perform are hidden instead of failing at the end.

| Connection | What you get |
| --- | --- |
| **MySQL / MariaDB** | Tables, views, stored functions and procedures, accounts and privileges, table designer and `.rgbak` backup. |
| **PostgreSQL** | Schemas, tables, views, functions/procedures, roles and ACLs, table designer and backup. Objects are schema-qualified. |
| **SQL Server** | Schemas, tables, views, procedures/functions, logins and roles, table designer and backup. Windows authentication is available in Windows builds. |
| **Oracle** | Schemas, tables, views, functions/procedures/packages/triggers, users and roles, designer and backup. Connect with a service name, SID or TNS alias. |
| **SQLite** | A single database file, including SQLCipher 4 and SQLCipher 3 encryption. No server accounts, routines or events. |
| **ClickHouse** | Databases, tables and views over the HTTP interface. The grid is read-only; the designer edits fields, defaults and comments. |
| **Redis** | Numbered databases, key namespaces, strings, hashes, lists, sets, sorted sets and streams. Every key opens its own page with TTL and memory. |
| **MongoDB** | Databases, collections and documents. One document is one grid row, `_id` is the key, and the row form has a JSON view. |
| **ODBC** | Any installed ODBC driver; table designer is disabled because a generic driver cannot reproduce indexes and options faithfully. |

## Notes

- **Capability-driven UI**: the Users, Functions, Backup or designer tabs only appear when the connected driver supports them.
- **Schema levels**: SQL Server, PostgreSQL and Oracle show database → schema → objects; MySQL, MariaDB and SQLite go straight from database to objects.
- **Read-only grids**: ClickHouse has no unique row key, so editing actions are hidden; MongoDB highlights `_id`, and Redis key pages have their own editors.
- **ODBC at runtime**: RustGrid loads the ODBC driver manager only when you use an ODBC connection. The generic driver still lists tables, runs SQL and pages data, but does not design tables.

See [Connections](./connections.html) for the fields each engine adds to the connection form.
