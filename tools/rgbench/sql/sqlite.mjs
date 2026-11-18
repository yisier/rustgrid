// RustGrid docs benchmark data set: SQLite (1M / 10M rows).
// Run with: node tools/rgbench/sql/sqlite.mjs
import { DatabaseSync } from "node:sqlite";
import { mkdirSync, rmSync } from "node:fs";
import { dirname, resolve } from "node:path";

const file = resolve("target/rgbench/rgbench.db");
mkdirSync(dirname(file), { recursive: true });
rmSync(file, { force: true });

const db = new DatabaseSync(file);
db.exec(`PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;`);
const schema = `CREATE TABLE NAME (
  id INTEGER PRIMARY KEY,
  user_id INTEGER NOT NULL,
  status TEXT NOT NULL,
  amount REAL NOT NULL,
  created_at TEXT NOT NULL,
  note TEXT NOT NULL
)`;

const statuses = ["paid", "pending", "shipped", "refunded", "cancelled"];
const pad = (n) => String(n).padStart(2, "0");
const stamp = (i) => {
  const s = i % 31536000;
  const d = new Date(Date.UTC(2025, 0, 1) + s * 1000);
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`;
};

function load(name, total) {
  db.exec(`DROP TABLE IF EXISTS ${name}`);
  db.exec(schema.replace("NAME", name));
  const stmt = db.prepare(`INSERT INTO ${name} (id, user_id, status, amount, created_at, note) VALUES (?, ?, ?, ?, ?, ?)`);
  db.exec("BEGIN");
  for (let i = 1; i <= total; i += 1) {
    stmt.run(i, i % 10000, statuses[i % 5], (i % 100000) / 100, stamp(i), `note-${i % 997}`);
    if (i % 1000000 === 0) console.log(`${name}: ${i} / ${total}`);
  }
  db.exec("COMMIT");
  db.exec(`CREATE INDEX idx_${name}_status_created ON ${name} (status, created_at)`);
  db.exec(`CREATE INDEX idx_${name}_user ON ${name} (user_id)`);
  db.exec(`ANALYZE ${name}`);
  console.log(`${name}: ${db.prepare(`SELECT COUNT(*) AS n FROM ${name}`).get().n} rows`);
}

load("orders_1m", 1_000_000);
load("orders_10m", 10_000_000);
db.exec("PRAGMA wal_checkpoint(TRUNCATE)");
db.close();
console.log(`sqlite file: ${file}`);
