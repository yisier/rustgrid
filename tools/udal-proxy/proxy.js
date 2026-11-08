#!/usr/bin/env node
// A MySQL-aware TCP proxy that reproduces UDAL/TeleDB DBProxy's answer to a client that
// uses server-side prepared statements:
//
//   1047 - UDAL - Unknown command
//
// It forwards the handshake and every COM_QUERY (text protocol) to a real MySQL server, but
// answers COM_STMT_PREPARE / COM_STMT_EXECUTE / COM_STMT_RESET with the UDAL error packet
// instead of forwarding them. That is exactly what UDAL does when its DBProxy has
// `enablePrepareStatement` off (or is an older build without the command): sqlx prepares even
// for a `SELECT` without parameters, so RustGrid fails when it opens a database while
// Navicat, which uses the text protocol, works.
//
// Usage (Node 18+, no npm dependencies):
//
//   1. Start a real MySQL/MariaDB (Docker is fine), e.g. 127.0.0.1:3306.
//   2. node tools/udal-proxy/proxy.js
//   3. In RustGrid create a MySQL connection to 127.0.0.1:13306.
//
// Environment variables:
//   LISTEN_HOST  (default 127.0.0.1)
//   LISTEN_PORT  (default 13306)
//   BACKEND_HOST (default 127.0.0.1)
//   BACKEND_PORT (default 3306)
//
// The proxy logs every rejected prepare, so the failing SQL is visible in its console.

const net = require('node:net');

const LISTEN_HOST = process.env.LISTEN_HOST ?? '127.0.0.1';
const LISTEN_PORT = Number(process.env.LISTEN_PORT ?? 13306);
const BACKEND_HOST = process.env.BACKEND_HOST ?? '127.0.0.1';
const BACKEND_PORT = Number(process.env.BACKEND_PORT ?? 3306);

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

function errorPacket(sequence, message) {
  const text = Buffer.from(message, 'utf8');
  const payload = Buffer.alloc(9 + text.length);
  payload[0] = 0xff;
  payload.writeUInt16LE(1047, 1);
  payload[3] = 0x23; // '#'
  payload.write('08S01', 4, 'ascii');
  text.copy(payload, 9);
  return frame(payload, sequence);
}

const server = net.createServer((client) => {
  const upstream = net.connect(BACKEND_PORT, BACKEND_HOST);
  let pending = Buffer.alloc(0);
  let closed = false;

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

      if (command === COM_STMT_PREPARE) {
        const sql = packet.subarray(5).toString('utf8');
        console.log(
          '[udal-proxy] reject COM_STMT_PREPARE:',
          sql.length > 160 ? `${sql.slice(0, 160)}...` : sql,
        );
        client.write(errorPacket(1, 'UDAL - Unknown command'));
        continue;
      }

      // COM_STMT_CLOSE and COM_STMT_SEND_LONG_DATA expect no response; drop them silently.
      if (command === COM_STMT_CLOSE || command === COM_STMT_SEND_LONG_DATA) {
        continue;
      }

      if (command === COM_STMT_EXECUTE || command === COM_STMT_RESET) {
        console.log('[udal-proxy] reject prepared-statement command', command);
        client.write(errorPacket(1, 'UDAL - Unknown command'));
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
    '[udal-proxy] COM_STMT_PREPARE is answered with 1047 "UDAL - Unknown command"',
  );
});
