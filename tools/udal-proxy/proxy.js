#!/usr/bin/env node
// A MySQL-aware TCP proxy that reproduces UDAL/TeleDB DBProxy's answers to a client:
//
//   1047 - UDAL - Unknown command          (COM_STMT_PREPARE)
//   1046 - UDAL - No database selected     (any statement while the session has no current
//                                           database, even a fully qualified information_schema
//                                           query)
//
// It forwards the handshake and every COM_QUERY (text protocol) to a real MySQL server, but
// answers COM_STMT_PREPARE / COM_STMT_EXECUTE / COM_STMT_RESET with the UDAL error packet
// instead of forwarding them. That is exactly what UDAL does when its DBProxy has
// `enablePrepareStatement` off (or is an older build without the command): sqlx prepares even
// for a `SELECT` without parameters, so RustGrid fails when it opens a database while Navicat,
// which uses the text protocol, works. A statement that arrives before the session has selected
// a database gets 1046, the second shape UDAL reports (issue #56) — the driver answers it by
// issuing `USE <database>` and retrying.
//
// Usage (Node 18+, no npm dependencies):
//
//   1. Start a real MySQL/MariaDB (Docker is fine), e.g. 127.0.0.1:3306.
//   2. node tools/udal-proxy/proxy.js
//   3. In RustGrid create a MySQL connection to 127.0.0.1:13306 and leave 数据库 empty.
//   4. Open a database: the table list, columns, pages and writes must all work.
//
// Environment variables:
//   LISTEN_HOST  (default 127.0.0.1)
//   LISTEN_PORT  (default 13306)
//   BACKEND_HOST (default 127.0.0.1)
//   BACKEND_PORT (default 3306)
//
// The proxy logs the statement it rejects (or the `USE` it forwards), so the fallback is visible
// in its console.

const net = require('node:net');

const LISTEN_HOST = process.env.LISTEN_HOST ?? '127.0.0.1';
const LISTEN_PORT = Number(process.env.LISTEN_PORT ?? 13306);
const BACKEND_HOST = process.env.BACKEND_HOST ?? '127.0.0.1';
const BACKEND_PORT = Number(process.env.BACKEND_PORT ?? 3306);

const COM_QUERY = 0x03;
const COM_INIT_DB = 0x02;
const COM_STMT_SEND_LONG_DATA = 0x18;
const COM_STMT_CLOSE = 0x19;
const COM_STMT_PREPARE = 0x16;
const COM_STMT_EXECUTE = 0x17;
const COM_STMT_RESET = 0x1a;

function frame(payload, sequence) {
  const packet = Buffer.alloc(4 + payload.length);
  packet.writeUIntLE(payload.length, 0, 3);
  packet[3] = sequence;
  payload.copy(packet, 4);
  return packet;
}

function errorPacket(sequence, code, state, message) {
  const text = Buffer.from(message, 'utf8');
  const payload = Buffer.alloc(9 + text.length);
  payload[0] = 0xff;
  payload.writeUInt16LE(code, 1);
  payload[3] = 0x23; // '#'
  payload.write(state, 4, 'ascii');
  text.copy(payload, 9);
  return frame(payload, sequence);
}

function shortSql(sql) {
  return sql.length > 160 ? `${sql.slice(0, 160)}...` : sql;
}

/** The database a `USE` statement selects, or null when `sql` is not one. */
function selectedDatabase(sql) {
  const match = /^\s*USE\s+(.+?)\s*;?\s*$/i.exec(sql);
  if (!match) return null;
  return match[1].replace(/[`"']/g, '');
}

/**
 * Whether UDAL's DBProxy refuses `sql` while the session has no current database. Session setup,
 * `SHOW DATABASES` and version probes must keep working before the first `USE`; everything else
 * (including the catalog `SHOW`s and `information_schema` queries) needs one.
 */
function requiresDatabase(sql) {
  const normalized = sql.trimStart().toUpperCase();
  return !(
    normalized.startsWith('USE ') ||
    normalized.startsWith('SET ') ||
    normalized.startsWith('SHOW DATABASES') ||
    normalized.startsWith('SHOW VARIABLES') ||
    normalized.startsWith('SHOW WARNINGS') ||
    normalized.startsWith('SHOW ERRORS') ||
    normalized.startsWith('SELECT VERSION()') ||
    normalized.startsWith('SELECT @@') ||
    normalized.startsWith('SELECT DATABASE()') ||
    normalized.startsWith('SELECT 1') ||
    normalized.startsWith('CREATE DATABASE') ||
    normalized.startsWith('DROP DATABASE') ||
    normalized.startsWith('ALTER DATABASE') ||
    normalized.startsWith('CREATE USER') ||
    normalized.startsWith('DROP USER') ||
    normalized.startsWith('GRANT ') ||
    normalized.startsWith('REVOKE ') ||
    normalized.startsWith('SET PASSWORD') ||
    normalized.startsWith('FLUSH ')
  );
}

const server = net.createServer((client) => {
  const upstream = net.connect(BACKEND_PORT, BACKEND_HOST);
  let pending = Buffer.alloc(0);
  let closed = false;
  // `USE` is connection state, so every client session tracks its own current database.
  let currentDatabase = null;

  const fail = (error) => {
    if (closed) return;
    closed = true;
    console.error('[udal-proxy]', error.message);
    client.destroy();
    upstream.destroy();
  };

  client.on('error', fail);
  upstream.on('error', fail);

  // Server -> client traffic is forwarded untouched (handshake, result sets, OK/ERR).
  upstream.pipe(client);

  client.on('data', (chunk) => {
    pending = Buffer.concat([pending, chunk]);

    while (pending.length >= 4) {
      const length = pending.readUIntLE(0, 3);
      if (pending.length < 4 + length) break;

      const packet = pending.subarray(0, 4 + length);
      pending = pending.subarray(4 + length);

      const sequence = packet[3];
      const command = packet[4];

      // Every new client command has sequence 0. The handshake response and an auth-switch
      // response carry other sequence ids, so they are forwarded without interpretation.
      if (sequence !== 0) {
        upstream.write(packet);
        continue;
      }

      if (command === COM_INIT_DB) {
        // COM_INIT_DB is what MariaDB Connector/C's mysql_select_db (and Navicat) sends. Track
        // it as the session's current database and forward it to the real server.
        const database = packet.subarray(5).toString('utf8');
        currentDatabase = database;
        console.log(`[udal-proxy] INIT_DB ${database}`);
        upstream.write(packet);
        continue;
      }

      if (command === COM_QUERY) {
        const sql = packet.subarray(5).toString('utf8');
        const database = selectedDatabase(sql);
        if (database !== null) {
          currentDatabase = database;
          console.log(`[udal-proxy] USE ${database}`);
          upstream.write(packet);
          continue;
        }
        if (currentDatabase === null && requiresDatabase(sql)) {
          console.log('[udal-proxy] 1046 (no database):', shortSql(sql));
          client.write(
            errorPacket(
              1,
              1046,
              'HY000',
              `UDAL - No database selected : ${sql}`,
            ),
          );
          continue;
        }
        upstream.write(packet);
        continue;
      }

      if (command === COM_STMT_PREPARE) {
        const sql = packet.subarray(5).toString('utf8');
        if (currentDatabase === null) {
          console.log('[udal-proxy] 1046 (no database):', shortSql(sql));
          client.write(
            errorPacket(
              1,
              1046,
              'HY000',
              `UDAL - No database selected : ${sql}`,
            ),
          );
        } else if (/information_schema\.tables/i.test(sql)) {
          // UDAL rewrites information_schema.tables to <current_database>.tables once a database
          // is selected; a client that retries the prepared catalog query then sees 3013.
          const message = `Table '${currentDatabase.toLowerCase()}.tables' doesn't exist.`;
          console.log('[udal-proxy] 3013:', message);
          client.write(errorPacket(1, 3013, 'HY000', message));
        } else {
          // UDAL accepts server-side prepares once a current database is selected (that is how
          // the prepared information_schema.tables probe reached its 3013 rewrite); forward the
          // statement so the designer and zero-row metadata use the real server metadata.
          console.log('[udal-proxy] forward COM_STMT_PREPARE:', shortSql(sql));
          upstream.write(packet);
        }
        continue;
      }

      // COM_STMT_CLOSE and COM_STMT_SEND_LONG_DATA expect no response; forward them so the
      // backend releases a statement it prepared. Before a database is selected there is no
      // valid statement id, so they can be dropped.
      if (command === COM_STMT_CLOSE || command === COM_STMT_SEND_LONG_DATA) {
        if (currentDatabase !== null) upstream.write(packet);
        continue;
      }

      if (command === COM_STMT_EXECUTE || command === COM_STMT_RESET) {
        if (currentDatabase !== null) {
          upstream.write(packet);
        } else {
          console.log('[udal-proxy] reject prepared-statement command', command);
          client.write(errorPacket(1, 1047, '08S01', 'UDAL - Unknown command'));
        }
        continue;
      }

      upstream.write(packet);
    }
  });

  client.on('close', () => upstream.destroy());
  upstream.on('close', () => client.destroy());
});

server.listen(LISTEN_PORT, LISTEN_HOST, () => {
  console.log(
    `[udal-proxy] ${LISTEN_HOST}:${LISTEN_PORT} -> ${BACKEND_HOST}:${BACKEND_PORT}`,
  );
  console.log(
    '[udal-proxy] a session without a database gets 1046; afterwards COM_STMT_PREPARE is forwarded (except information_schema.tables, which gets 3013)',
  );
});
