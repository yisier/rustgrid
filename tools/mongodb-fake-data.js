// ============================================================================
// RustGrid MongoDB sample data.
//
// Seeds two databases on a throwaway/test MongoDB server:
//
//   appdb.users      300 documents  —— 中文姓名/地址、ObjectId、Decimal128、NumberLong、
//                                     Date、数组、嵌套文档、可为 null 的字段
//   appdb.products   120 documents  —— 商品、分类、价格、库存、属性、内嵌评价数组
//   appdb.orders    1200 documents  —— 引用 users/_id 与商品 sku、内嵌明细数组、
//                                     金额 Decimal128、支付/物流子文档、状态时间线
//   appdb.events    2000 documents  —— 日志型数据（类型/级别/路径/IP/payload/耗时）
//   appdb.vip_users                 —— 基于 users 的视图（付费会员）
//   test.type_showcase  1 document  —— BSON 类型大全（ObjectId/Date/Timestamp/
//                                     Decimal128/UUID/Binary/Regex/MinKey/MaxKey…）
//   test.people        50 documents —— 字段可有可无的小集合，用于测试采样列
//
// `rg_placeholder` 和其它数据库不会被删除；脚本只重建自己拥有的集合，可重复执行。
//
// Usage (Windows -> WSL docker):
//   wsl -d Ubuntu-24.04 -- docker cp tools/mongodb-fake-data.js db-mongodb:/tmp/seed.js
//   wsl -d Ubuntu-24.04 -- docker exec db-mongodb mongosh -u root -p 'Root123!' \
//       --authenticationDatabase admin --file /tmp/seed.js
//
// Or from a mongosh prompt:
//   load("tools/mongodb-fake-data.js")
//
// Counts can be changed at the top; the generator is deterministic (same seed
// produces the same data), so the output is reproducible.
// ============================================================================

const APP_DB = "appdb";
const TEST_DB = "test";

const USERS = 300;
const PRODUCTS = 120;
const ORDERS = 1200;
const EVENTS = 2000;
const PEOPLE = 50;

const SURNAMES = ["张", "王", "李", "赵", "刘", "陈", "杨", "黄", "周", "吴"];
const GIVEN = [
  "伟", "芳", "娜", "敏", "静", "强", "磊", "洋", "艳", "勇",
  "军", "杰", "娟", "涛", "明", "超", "秀英", "霞", "平", "刚",
];
const PLACES = [
  ["广东", "广州"], ["广东", "深圳"], ["浙江", "杭州"], ["江苏", "南京"],
  ["四川", "成都"], ["湖北", "武汉"], ["湖南", "长沙"], ["安徽", "合肥"],
  ["福建", "福州"], ["山东", "济南"],
];
const TAG_POOL = [
  "新客", "活跃", "高价值", "沉睡", "企业", "个人",
  "复购", "优惠敏感", "移动端", "桌面端",
];
const CATEGORIES = [
  ["手机", "华为"], ["手机", "小米"], ["手机", "苹果"],
  ["笔记本", "联想"], ["笔记本", "戴尔"],
  ["平板", "苹果"], ["平板", "华为"],
  ["耳机", "索尼"], ["耳机", "苹果"],
  ["显示器", "戴尔"], ["显示器", "三星"],
  ["键盘", "罗技"], ["鼠标", "罗技"], ["路由器", "华为"],
  ["音箱", "小米"], ["摄像头", "海康威视"],
];
const COLORS = ["曜石黑", "珍珠白", "星空蓝", "远峰蓝", "晨曦金", "松石绿", "银灰"];
const STATUSES = ["pending", "paid", "shipped", "completed", "cancelled", "refunded"];
const PAYMENTS = ["微信支付", "支付宝", "银行卡", "货到付款"];
const EVENT_TYPES = [
  "page_view", "login", "logout", "search", "add_to_cart",
  "purchase", "payment", "api_call", "error",
];
const LEVELS = ["debug", "info", "warn", "error"];
const USER_AGENTS = [
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/131.0",
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 14_1) Safari/605.1",
  "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Mobile/15E148",
  "Mozilla/5.0 (Linux; Android 15; Pixel 9) Chrome/131.0",
];

// ---- deterministic pseudo-random helpers -----------------------------------

let rngState = 20261108;
function rnd() {
  rngState = (rngState * 1664525 + 1013904223) >>> 0;
  return rngState / 4294967296;
}
function pick(list) {
  return list[Math.floor(rnd() * list.length)];
}
function int(min, max) {
  return min + Math.floor(rnd() * (max - min + 1));
}
function chance(p) {
  return rnd() < p;
}
function pad(value, width) {
  return String(value).padStart(width, "0");
}
function money(min, max) {
  return (min + rnd() * (max - min)).toFixed(2);
}
function dec(text) {
  return Decimal128.fromString(text);
}
const BASE_TIME = new Date("2024-01-01T00:00:00Z").getTime();
const TWO_YEARS = 730 * 24 * 3600 * 1000;
function when() {
  return new Date(BASE_TIME + Math.floor(rnd() * TWO_YEARS));
}
function after(date, maxHours) {
  return new Date(date.getTime() + int(1, maxHours) * 3600 * 1000);
}
function unique(list) {
  return [...new Set(list)];
}
function insertAll(collection, documents, batchSize) {
  const size = batchSize || 500;
  for (let i = 0; i < documents.length; i += size) {
    collection.insertMany(documents.slice(i, i + size), { ordered: false });
  }
}

// ---- appdb ----------------------------------------------------------------

const app = db.getSiblingDB(APP_DB);
for (const name of ["users", "products", "orders", "events", "vip_users"]) {
  app.getCollection(name).drop();
}

print("生成 products ...");
const products = [];
for (let i = 1; i <= PRODUCTS; i++) {
  const [category, brand] = pick(CATEGORIES);
  const priceNumber = Number(money(29, 9999));
  const reviews = [];
  for (let r = 0; r < int(0, 3); r++) {
    reviews.push({
      user: pick(SURNAMES) + "**",
      stars: NumberInt(int(1, 5)),
      comment: pick([
        "和描述一致，物流很快。",
        "包装有点破损，东西没问题。",
        "第二次回购了，推荐。",
        "性价比不错，用起来顺手。",
        "客服响应及时，满意。",
      ]),
      at: when(),
    });
  }
  products.push({
    _id: new ObjectId(),
    sku: "SKU-" + pad(i, 6),
    name: brand + " " + category + " " + pick(["Pro", "Max", "Air", "Plus", "Lite", "旗舰版"]) + " " + int(1, 99),
    category,
    brand,
    price: dec(priceNumber.toFixed(2)),
    cost: Double(Math.round(priceNumber * (0.55 + rnd() * 0.2) * 100) / 100),
    stock: NumberInt(int(0, 500)),
    on_sale: chance(0.85),
    rating: Double(Math.round((3 + rnd() * 2) * 10) / 10),
    tags: unique(Array.from({ length: int(0, 3) }, () => pick(TAG_POOL))),
    attributes: {
      color: pick(COLORS),
      size: pick(["标准版", "大号", "小号", "256G", "512G", "1T"]),
      weight_g: NumberInt(int(100, 5000)),
      warranty_months: NumberInt(pick([12, 24, 36])),
    },
    reviews,
    created_at: when(),
    updated_at: when(),
  });
}
insertAll(app.products, products);

print("生成 users ...");
const users = [];
for (let i = 1; i <= USERS; i++) {
  const [province, city] = pick(PLACES);
  const created = when();
  const vip = chance(0.15);
  const name = pick(SURNAMES) + pick(GIVEN);
  const longBio = [
    "长期从事互联网产品与数据平台建设，关注数据库工具链、可观测性和自动化运维。",
    "喜欢开源软件，业余时间维护几个小型项目，也在学习 Rust 和分布式系统。",
    "做电商运营，日常需要查订单、导出报表，偶尔写 SQL 做数据分析。",
  ];
  users.push({
    _id: new ObjectId(),
    username: "user" + pad(i, 4),
    name: name,
    email: pick(["user", "test", "demo", "buyer"]) + i + "@" + pick(["example.com", "gmail.com", "qq.com", "163.com"]),
    phone: "1" + pick(["38", "39", "58", "86", "88"]) + pad(int(0, 99999999), 8),
    gender: pick(["男", "女"]),
    age: NumberInt(int(18, 65)),
    active: chance(0.8),
    vip: vip,
    vip_since: vip ? after(created, 24 * 90) : null,
    balance: dec(money(0, 100000)),
    login_count: NumberLong(String(int(0, 5000))),
    rating: Double(Math.round(rnd() * 50) / 10),
    tags: unique(Array.from({ length: int(0, 4) }, () => pick(TAG_POOL))),
    address: {
      country: "中国",
      province: province,
      city: city,
      street: "科技园" + pick(["一", "二", "三", "四", "五", "六", "七", "八", "九", "十"]) + "路 " + int(1, 200) + " 号",
      zip: pad(int(0, 999999), 6),
    },
    profile: {
      avatar: "https://cdn.example.com/avatar/" + i + ".png",
      website: chance(0.3) ? "https://home.example.com/" + i : null,
      bio: chance(0.3) ? pick(longBio) : "",
      social: {
        wechat: chance(0.6) ? "wx_" + pad(i, 6) : null,
        weibo: chance(0.2) ? "@user" + pad(i, 4) : null,
      },
    },
    preferences: {
      language: pick(["zh-CN", "en-US"]),
      timezone: pick(["Asia/Shanghai", "Asia/Tokyo", "UTC"]),
      notifications: { email: chance(0.7), sms: chance(0.4), push: chance(0.5) },
    },
    last_login_at: after(created, 24 * 30),
    created_at: created,
    deleted_at: null,
  });
}
insertAll(app.users, users);

print("生成 orders ...");
const orders = [];
for (let i = 1; i <= ORDERS; i++) {
  const user = pick(users);
  const itemCount = int(1, 4);
  const items = [];
  let subtotalNumber = 0;
  for (let j = 0; j < itemCount; j++) {
    const product = pick(products);
    const qty = int(1, 5);
    const unitPrice = Number(product.price.toString());
    const amount = unitPrice * qty;
    subtotalNumber += amount;
    items.push({
      sku: product.sku,
      name: product.name,
      category: product.category,
      qty: NumberInt(qty),
      unit_price: product.price,
      amount: dec(amount.toFixed(2)),
    });
  }
  const status = pick(STATUSES);
  const created = when();
  const paid = status !== "pending" && status !== "cancelled";
  const shipped = status === "shipped" || status === "completed" || (status === "refunded" && chance(0.5));
  const discountNumber = chance(0.4) ? Math.round(subtotalNumber * rnd() * 0.15 * 100) / 100 : 0;
  const totalNumber = subtotalNumber - discountNumber;
  const [province, city] = pick(PLACES);
  const paidAt = paid ? after(created, 24) : null;
  orders.push({
    _id: new ObjectId(),
    order_no: "ORD" + pad(i, 10),
    user_id: user._id,
    user_name: user.name,
    status: status,
    items: items,
    item_count: NumberInt(itemCount),
    subtotal: dec(subtotalNumber.toFixed(2)),
    discount: dec(discountNumber.toFixed(2)),
    total: dec(totalNumber.toFixed(2)),
    currency: "CNY",
    paid: paid,
    paid_at: paidAt,
    shipped_at: shipped ? after(paidAt || created, 96) : null,
    payment: paid
      ? {
          method: pick(PAYMENTS),
          transaction_id: "TX" + pad(int(100000, 999999), 6),
          amount: dec(totalNumber.toFixed(2)),
        }
      : null,
    shipping: {
      receiver: user.name,
      phone: user.phone,
      province: province,
      city: city,
      street: "科技园" + pick(["一", "二", "三", "四"]) + "路 " + int(1, 200) + " 号 " + int(1, 30) + " 栋",
      zip: pad(int(0, 999999), 6),
    },
    remark: chance(0.2) ? "客户备注：" + pick(["尽快发货", "工作日送达", "需要发票", "放前台", "电话联系"]) : "",
    created_at: created,
    updated_at: after(created, 72),
  });
}
insertAll(app.orders, orders);

print("生成 events ...");
const events = [];
for (let i = 1; i <= EVENTS; i++) {
  const type = pick(EVENT_TYPES);
  const level = type === "error" ? "error" : pick(LEVELS);
  const user = chance(0.7) ? pick(users) : null;
  events.push({
    _id: new ObjectId(),
    user_id: user ? user._id : null,
    username: user ? user.username : null,
    type: type,
    level: level,
    message:
      type === "error"
        ? pick(["连接超时", "订单不存在", "库存不足", "支付回调验签失败", "查询参数非法"])
        : type + " " + pick(["/api/orders", "/api/products", "/api/users", "/api/checkout", "/api/search"]),
    path: pick(["/", "/products", "/orders", "/users", "/search"]),
    http_status: NumberInt(pick([200, 200, 200, 201, 400, 404, 500])),
    ip: "10." + int(0, 255) + "." + int(0, 255) + "." + int(1, 254),
    user_agent: pick(USER_AGENTS),
    payload: {
      order_no: chance(0.4) ? "ORD" + pad(int(1, ORDERS), 10) : null,
      sku: chance(0.4) ? pick(products).sku : null,
      keyword: chance(0.3) ? pick(["耳机", "手机", "键盘", "显示器", "路由器"]) : null,
      duration_ms: NumberInt(int(1, 3000)),
    },
    at: when(),
    duration_ms: NumberInt(int(1, 3000)),
  });
}
insertAll(app.events, events);

print("创建索引和视图 ...");
app.users.createIndex({ username: 1 }, { unique: true, name: "ux_username" });
app.users.createIndex({ email: 1 }, { name: "ix_email" });
app.users.createIndex({ created_at: -1 }, { name: "ix_created_at" });
app.users.createIndex({ "address.city": 1 }, { name: "ix_city" });
app.products.createIndex({ sku: 1 }, { unique: true, name: "ux_sku" });
app.products.createIndex({ category: 1, price: -1 }, { name: "ix_category_price" });
app.orders.createIndex({ order_no: 1 }, { unique: true, name: "ux_order_no" });
app.orders.createIndex({ user_id: 1, created_at: -1 }, { name: "ix_user_created" });
app.orders.createIndex({ status: 1 }, { name: "ix_status" });
app.orders.createIndex({ created_at: -1 }, { name: "ix_created_at" });
app.events.createIndex({ at: -1 }, { name: "ix_at" });
app.events.createIndex({ type: 1, at: -1 }, { name: "ix_type_at" });
app.createView("vip_users", "users", [
  { $match: { vip: true } },
  {
    $project: {
      username: 1,
      name: 1,
      email: 1,
      balance: 1,
      tags: 1,
      address: 1,
      vip_since: 1,
      created_at: 1,
    },
  },
]);

// ---- test ----------------------------------------------------------------

const test = db.getSiblingDB(TEST_DB);
test.getCollection("type_showcase").drop();
test.getCollection("people").drop();

print("生成 test.type_showcase ...");
test.type_showcase.insertOne({
  _id: new ObjectId("65f0c0ffee00000000000001"),
  title: "BSON 类型演示",
  string: "文本 / 中文 / emoji ✅",
  int32: NumberInt(42),
  int64: NumberLong("9007199254740993"),
  double: Double(3.14159),
  decimal: Decimal128.fromString("1234567890.123456789"),
  bool_true: true,
  bool_false: false,
  null_value: null,
  date: new Date("2026-11-08T12:34:56Z"),
  timestamp: Timestamp({ t: 1762600496, i: 1 }),
  object_id: ObjectId("65f0c0ffee00000000000002"),
  uuid: UUID("00112233-4455-6677-8899-aabbccddeeff"),
  binary: BinData(0, "SGVsbG8sIE1vbmdvREIh"),
  regex: /^user-\d+$/i,
  array: [1, "two", true, null, { nested: 1 }],
  nested: {
    level1: { level2: { key: "value", count: NumberInt(2) } },
    city: "杭州",
  },
  long_text:
    "这是一段超过 80 个字符的长文本，用来验证 RustGrid 行表单的多行编辑器、滚动和格式化显示。" +
    "第二句：包含换行。\n" +
    "第三句：包含 emoji ✅ 和中文标点，结束。",
  min_key: MinKey(),
  max_key: MaxKey(),
});

print("生成 test.people ...");
const people = [];
for (let i = 1; i <= PEOPLE; i++) {
  const city = pick(PLACES);
  const person = {
    _id: new ObjectId(),
    name: pick(SURNAMES) + pick(GIVEN),
    age: NumberInt(int(18, 65)),
    email: "person" + i + "@example.com",
    city: city[1],
    tags: unique(Array.from({ length: int(0, 3) }, () => pick(TAG_POOL))),
    active: chance(0.7),
    score: Double(Math.round(rnd() * 1000) / 10),
    joined_at: when(),
  };
  // 只给一部分文档加 address/manager，让列采样与可空单元格都有东西可测。
  if (chance(0.5)) {
    person.address = {
      country: "中国",
      province: city[0],
      city: city[1],
      street: "示例路 " + int(1, 200) + " 号",
    };
  }
  if (chance(0.3)) {
    person.manager = { name: pick(SURNAMES) + pick(GIVEN), email: "boss" + int(1, 5) + "@example.com" };
  }
  people.push(person);
}
insertAll(test.people, people);
test.people.createIndex({ email: 1 }, { unique: true, name: "ux_email" });
test.people.createIndex({ city: 1 }, { name: "ix_city" });

// ---- summary --------------------------------------------------------------

print("");
print("完成：");
for (const database of [APP_DB, TEST_DB]) {
  const handle = db.getSiblingDB(database);
  const names = handle
    .getCollectionNames()
    .filter((name) => !name.startsWith("system."))
    .sort();
  for (const name of names) {
    let count = "?";
    try {
      count = handle.getCollection(name).countDocuments({});
    } catch (error) {
      count = "读取失败";
    }
    print("  " + database + "." + name + "  " + count + " 条");
  }
}
print("");
print("连接：127.0.0.1:27017  用户 root  密码 Root123!  认证库 admin");
