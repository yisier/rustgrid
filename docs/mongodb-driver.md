# MongoDB driver (`rustgrid-mongodb`)

MongoDB is RustGrid's **document-store** engine. A connection is still a server → databases →
objects tree, but a "table" is a **collection** and a row is a **document**. The app treats the
driver through the same `Driver` / `Connection` traits as the SQL engines, with one extra
capability (`DriverCapability::Documents`) that only changes how the UI *labels* and *gates* things:
the data grid, WHERE filter, sorting, pagination and the row form are the existing ones.

## Stack

- The official Rust driver **`mongodb` 3.10** (`default-features = false`), with `bson-3` and
  `rustls-tls` only. DNS/SRV resolution, compression (snappy/zstd/zlib) and the cloud KMS
  integrations are deliberately off to keep the dependency tree and the binary small.
- TLS reuses the workspace's rustls (the CA certificate and client certificate/key paths from the
  connection's TLS page become `TlsOptions`).
- SSH/SOCKS5/HTTP tunnels reuse `rustgrid-tunnel`, exactly like the Redis and SQL drivers: the
  client connects to `127.0.0.1:<tunnel port>`.

## Connection form

| Field | Option key | Page |
| --- | --- | --- |
| Default database (standard database field) | — | 常规 |
| Auth database | `mongodb.auth_source` (default `admin`) | 常规 |
| Authentication (`SCRAM` / `None`) | `mongodb.auth_mechanism` | 常规 |
| Replica set | `mongodb.replica_set` | 高级 |
| Read preference | `mongodb.read_preference` (primary … nearest) | 高级 |

TLS, tunnel, connect timeout and read-only come from the standard pages. A wrong password maps to
`Error::Authentication`, so the app prompts for the password like it does for the SQL engines.

## Data model

- `list_databases` hides `admin`, `config` and `local`; `list_tables` lists collections and views
  (`system.*` is skipped).
- `columns` samples up to 50 documents and takes the union of the **top-level** fields. `_id` is
  always the first column and is marked `primary_key`, so every edit/delete is addressed by `_id`.
- The sampled field set is cached per connection and dropped after an insert, so a document that
  adds a new field appears on the next reload.
- Cells: scalars keep their native `CellValue` type where possible; `ObjectId`, `Decimal128`,
  `DateTime`, `Timestamp`, `Binary` and embedded documents/arrays render as text/Extended JSON and
  parse back through the column's BSON type when edited.

| BSON | Grid cell |
| --- | --- |
| string / ObjectId / Decimal128 / date / timestamp / regex | text |
| int32 / int64 | number |
| double | number |
| bool | boolean |
| binary | hex bytes (edits parse back as subtype 0) |
| document / array | compact Extended JSON (the row form shows it in its JSON editor) |
| null / undefined | NULL |

Editing a `_id` cell is ignored (MongoDB keys are immutable); every other cell becomes a `$set`.

## Query tab

A query is **one JSON command object**, not JavaScript. The editor uses the JSON language, has no
SQL completion, and the result is shown in the normal result grid.

```json
{ "find": "users", "filter": { "age": { "$gte": 18 } }, "sort": { "age": -1 }, "limit": 50 }
{ "aggregate": "orders", "pipeline": [ { "$group": { "_id": "$status", "n": { "$sum": 1 } } } ] }
{ "count": "users", "query": { "active": true } }
{ "distinct": "users", "key": "address.city" }
{ "listCollections": 1 }
{ "create": "notes" }
{ "ping": 1 }
```

Anything that is not one of the recognized read commands is passed to `runCommand` on the selected
database and its reply is rendered as one row. Extended JSON (`{"$oid": …}`, `{"$date": …}`,
`{"$numberDecimal": …}`) is accepted everywhere. Result sets are capped at 5 000 documents.

## Capabilities and scope

`DriverCapability::Documents` + `DatabaseManagement`:

- No table designer, no `.rgbak` backup, no users/roles/routines, no Views category (collection
  views are listed among the collections themselves).
- `create_database` reports that MongoDB creates a database on the first write; collections are
  created with `{"create": "name"}` in the query tab until a document-specific designer exists.
- Import/export wizards run through the generic `fetch_page`/`insert_rows` path for existing
  collections; creating a table from the import wizard is not supported (it needs the SQL designer).

## Live test

```bash
RUSTGRID_MONGODB_PASSWORD='Root123!' cargo test -p rustgrid-mongodb -- --ignored
```

Optional: `RUSTGRID_MONGODB_HOST`/`PORT`/`USER`/`DATABASE`. The test creates and drops its own
`rustgrid_live` database; the second test reads the `test.type_showcase` fixture written by
`DATABASES.local.md`'s seed script.
