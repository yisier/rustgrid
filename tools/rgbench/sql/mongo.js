// RustGrid docs benchmark data set: MongoDB.
// Run with:
//   docker exec -i db-mongodb mongosh -u root -p 'Root123!' --authenticationDatabase admin --quiet < mongo.js
const db = db.getSiblingDB("rgbench");
const statuses = ["paid", "pending", "shipped", "refunded", "cancelled"];
const base = new Date("2025-01-01T00:00:00Z").getTime();

function makeDoc(i) {
  return {
    _id: NumberLong(i),
    user_id: NumberInt(i % 10000),
    status: statuses[i % 5],
    amount: (i % 100000) / 100,
    created_at: new Date(base + (i % 31536000) * 1000),
    note: "note-" + (i % 997),
  };
}

function load(name, total) {
  db[name].drop();
  const BATCH = 10000;
  for (let start = 1; start <= total; start += BATCH) {
    const docs = [];
    const end = Math.min(start + BATCH - 1, total);
    for (let i = start; i <= end; i += 1) docs.push(makeDoc(i));
    db[name].insertMany(docs, { ordered: false });
    if (start === 1 || start % 1000000 === 0) print(name + ": " + (end) + " / " + total);
  }
  db[name].createIndex({ status: 1, created_at: -1 });
  db[name].createIndex({ user_id: 1 });
  print(name + " count=" + db[name].countDocuments({}));
}

const total = NumberLong(Number(process.env.ROWS || 1000000));
load("orders_1m", 1000000);
load("orders_10m", 10000000);
print("done");
