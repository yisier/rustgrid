<script setup>
import { computed, ref } from 'vue'
import { usePageData } from 'vuepress/client'
import bench from '../data/bench.json'
import releases from '../data/releases.json'

const page = usePageData()
const zh = computed(() => page.value.path.startsWith('/zh/'))
const t = (en, cn) => (zh.value ? cn : en)

const ENGINE_LABELS = {
  mysql: 'MySQL',
  postgresql: 'PostgreSQL',
  clickhouse: 'ClickHouse',
  mongodb: 'MongoDB',
  oracle: 'Oracle',
  sqlite: 'SQLite',
}
const ENGINE_ORDER = ['mysql', 'postgresql', 'clickhouse', 'mongodb', 'oracle', 'sqlite']
const ENGINE_COLORS = {
  mysql: '#0891b2',
  postgresql: '#3b82f6',
  clickhouse: '#ca8a04',
  mongodb: '#16a34a',
  oracle: '#dc2626',
  sqlite: '#64748b',
}

const engines = computed(() =>
  ENGINE_ORDER.filter((id) => bench.engines?.[id]).map((id) => ({
    id,
    label: ENGINE_LABELS[id],
    version: bench.engines[id].version,
  })),
)

const dataset = ref('orders_10m')
const datasetLabel = (key) =>
  key === 'orders_10m' ? t('10,000,000 rows', '1,000 万行') : t('1,000,000 rows', '100 万行')

const metrics = computed(() => {
  const list = [
    {
      key: 'first_page_ms',
      name: t('First page · 1,000 rows', '首屏 · 1,000 行'),
      hint: t('rows only; COUNT is separate', '仅取行，总数为独立操作'),
      short: t('First page', '首屏加载'),
    },
    {
      key: 'deep_100k_ms',
      name: t('Page 101 · offset 100,000', '第 101 页 · 偏移 100,000'),
      hint: t('deep pagination', '深翻页'),
      short: t('Offset 100k', '深翻页 10 万'),
    },
    ...(dataset.value === 'orders_10m'
      ? [
          {
            key: 'deep_1m_ms',
            name: t('Page 1,001 · offset 1,000,000', '第 1,001 页 · 偏移 1,000,000'),
            hint: t('deep pagination', '深翻页'),
            short: t('Offset 1M', '深翻页 100 万'),
          },
        ]
      : []),
    {
      key: 'point_ms',
      name: t('Primary-key lookup', '主键点查'),
      hint: t('average of 30 lookups', '30 次平均'),
      short: t('Point lookup', '主键点查'),
    },
    {
      key: 'filtered_ms',
      name: t('Filter + sort · 100 rows', '过滤 + 排序 · 100 行'),
      hint: t('status = paid, newest first', 'status = paid，按时间倒序'),
      short: t('Filter + sort', '过滤排序'),
    },
    {
      key: 'count_ms',
      name: 'COUNT(*)',
      hint: t('full table', '全表'),
      short: 'COUNT(*)',
    },
  ]
  return list
})

const tableData = computed(() => bench.engines?.[dataset.value] ?? {})
const tableRows = computed(() => bench.engines?.[dataset.value]?.tables?.[dataset.value]?.ops ?? {})

function valueFor(engineId, key) {
  const tables = bench.engines?.[engineId]?.tables?.[dataset.value]
  return tables?.ops?.[key] ?? null
}

function bestOf(key) {
  let best = Infinity
  for (const engine of engines.value) {
    const value = valueFor(engine.id, key)
    if (typeof value === 'number' && value > 0 && value < best) best = value
  }
  return best
}

function barsFor(key) {
  const rows = engines.value
    .map((engine) => ({ ...engine, value: valueFor(engine.id, key) }))
    .filter((row) => typeof row.value === 'number')
  const max = Math.max(...rows.map((row) => row.value), 0.001)
  return rows.map((row) => ({
    ...row,
    width: Math.max(3, (row.value / max) * 100),
  }))
}

function fmtMs(value) {
  if (typeof value !== 'number') return '—'
  if (value < 10) return value.toFixed(2)
  if (value < 100) return value.toFixed(1)
  return Math.round(value).toLocaleString('en-US')
}

function versionLabel(engine) {
  if (engine.id === 'oracle') {
    const match = engine.version.match(/\d+(\.\d+){2,}/)
    return match ? match[0] : engine.version
  }
  return engine.version
}

function fmtInt(value) {
  return typeof value === 'number' ? value.toLocaleString('en-US') : '—'
}

function fmtMb(bytes) {
  return (bytes / 1_000_000).toFixed(1)
}

const sizes = computed(() => {
  const rows = [...(releases.assets ?? [])].sort((a, b) => a.bytes - b.bytes)
  const max = Math.max(...rows.map((row) => row.bytes), 1)
  return rows.map((row) => ({
    ...row,
    mb: fmtMb(row.bytes),
    width: Math.max(4, (row.bytes / max) * 100),
  }))
})

const windowsSize = computed(() => {
  const row = (releases.assets ?? []).find((asset) => asset.file.includes('windows-x64'))
  return row ? fmtMb(row.bytes) : '—'
})
const runtime = computed(() => releases.runtime ?? {})
const startupText = computed(() => {
  const value = runtime.value.startup_ms
  if (!value) return null
  return value >= 1000 ? `${(value / 1000).toFixed(2)} s` : `${Math.round(value)} ms`
})
const memoryText = computed(() => {
  const value = runtime.value.idle_memory_mb
  return value ? `${Math.round(value)} MB` : null
})

const totalRows = computed(() => {
  const rows = bench.engines?.['mysql']?.tables?.[dataset.value]?.rows
  return fmtInt(rows)
})
</script>

<template>
  <div class="report">
    <section class="block" id="size">
      <div class="block-head">
        <h2>{{ t('Size', '体积') }}</h2>
        <span class="muted">{{ t(`release ${releases.version}`, `版本 ${releases.version}`) }}</span>
      </div>
      <p class="lede">
        {{ t(
          'The complete desktop build ships as one native executable — no runtime, no Electron.',
          '完整桌面端只有一个原生可执行文件，不带运行时，也不是 Electron。'
        ) }}
      </p>
      <div class="size-grid">
        <div class="stat">
          <span class="stat-value">{{ windowsSize }}</span>
          <span class="stat-unit">MB</span>
          <span class="stat-label">{{ t('Windows x64 download', 'Windows x64 安装包') }}</span>
        </div>
        <div class="size-bars">
          <div v-for="row in sizes" :key="row.file" class="bar-row">
            <span class="bar-label">{{ row.platform }}</span>
            <span class="bar-track"><span class="bar-fill" :style="{ width: `${row.width}%` }" /></span>
            <span class="bar-value">{{ row.mb }} MB</span>
          </div>
        </div>
      </div>
      <div v-if="runtime.installed_mb || startupText || memoryText" class="runtime">
        <div v-if="runtime.installed_mb" class="runtime-item">
          <span class="runtime-label">{{ t('Installed', '安装后') }}</span>
          <b>{{ runtime.installed_mb }} MB</b>
          <span class="muted">{{ t('one executable', '单个可执行文件') }}</span>
        </div>
        <div v-if="startupText" class="runtime-item">
          <span class="runtime-label">{{ t('Cold start', '冷启动') }}</span>
          <b>{{ startupText }}</b>
          <span class="muted">{{ t('until the window appears', '到窗口出现') }}</span>
        </div>
        <div v-if="memoryText" class="runtime-item">
          <span class="runtime-label">{{ t('Idle memory', '空闲内存') }}</span>
          <b>{{ memoryText }}</b>
          <span class="muted">{{ t('working set', '工作集') }}</span>
        </div>
      </div>
      <p class="footnote">
        {{ t(
          'Archive (zip / tar.gz) sizes from the GitHub release. Unpacked, the Windows build is a single RustGrid.exe.',
          '以上为 GitHub Release 中压缩包（zip / tar.gz）的大小；解压后 Windows 版只有一个 RustGrid.exe。'
        ) }}
      </p>
    </section>

    <section class="block" id="benchmark">
      <div class="block-head">
        <h2>{{ t('Performance', '性能') }}</h2>
        <span class="muted">{{ t('grid operations, lower is better', '网格操作，越低越好') }}</span>
      </div>
      <p class="lede">
        {{ t(
          'Every number is a grid operation through RustGrid’s own driver stack, on the same six-column table and the same machine.',
          '所有数字都通过 RustGrid 自己的驱动执行，使用同一张六列表结构和同一台机器。'
        ) }}
      </p>
      <p class="footnote">
        {{ t(
          'RustGrid paints the page first and counts the total in a separate task, so first-page time excludes COUNT(*).',
          'RustGrid 先显示当页数据，再在后台统计总数，因此首屏时间不含 COUNT(*)。'
        ) }}
      </p>

      <div class="dataset-toggle">
        <button
          v-for="key in ['orders_1m', 'orders_10m']"
          :key="key"
          type="button"
          :class="{ active: dataset === key }"
          @click="dataset = key"
        >
          {{ datasetLabel(key) }}
        </button>
        <span class="muted rows">{{ t(`${totalRows} rows`, `${totalRows} 行`) }}</span>
      </div>

      <div v-if="engines.length" class="metric-grid">
        <div v-for="metric in metrics" :key="metric.key" class="metric-card">
          <div class="metric-head">
            <div>
              <div class="metric-name">{{ metric.name }}</div>
              <div class="metric-hint">{{ metric.hint }}</div>
            </div>
          </div>
          <div class="metric-bars">
            <div v-for="row in barsFor(metric.key)" :key="row.id" class="bar-row">
              <span class="bar-label engine">
                <i class="dot" :style="{ background: ENGINE_COLORS[row.id] }" />
                {{ row.label }}
                <em v-if="valueFor(row.id, metric.key) === bestOf(metric.key)" class="fastest">
                  {{ t('fastest', '最快') }}
                </em>
              </span>
              <span class="bar-track">
                <span
                  class="bar-fill"
                  :style="{ width: `${row.width}%`, background: ENGINE_COLORS[row.id] }"
                />
              </span>
              <span class="bar-value">{{ fmtMs(row.value) }} ms</span>
            </div>
          </div>
        </div>
      </div>
      <p v-else class="footnote">{{ t('Run tools/rgbench to fill this report.', '运行 tools/rgbench 生成数据。') }}</p>

      <details class="all-ops">
        <summary>{{ t('All operations', '完整数据') }}</summary>
        <div class="table-wrap">
          <table>
            <thead>
              <tr>
                <th>{{ t('Engine', '数据库') }}</th>
                <th v-for="metric in metrics" :key="metric.key">{{ metric.short }}</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="engine in engines" :key="engine.id">
                <td>
                  <i class="dot" :style="{ background: ENGINE_COLORS[engine.id] }" />
                  {{ engine.label }} <span class="muted" :title="engine.version">{{ versionLabel(engine) }}</span>
                </td>
                <td v-for="metric in metrics" :key="metric.key" class="num">
                  {{ fmtMs(valueFor(engine.id, metric.key)) }}
                </td>
              </tr>
            </tbody>
          </table>
        </div>
      </details>

      <div class="env">
        <div class="env-item"><b>{{ t('Machine', '机器') }}</b> Windows 11 · WSL2 · 12 GB RAM · 6 vCPU</div>
        <div class="env-item"><b>{{ t('Table', '表结构') }}</b> id, user_id, status, amount, created_at, note</div>
        <div class="env-item"><b>{{ t('Data', '数据') }}</b> 1,000,000 / 10,000,000 rows · index (status, created_at)</div>
        <div class="env-item"><b>{{ t('Engines', '数据库') }}</b> {{ engines.map((e) => e.label).join(' · ') }}</div>
        <div class="env-item"><b>{{ t('Method', '方法') }}</b> 1 warm-up + 3 runs, median · <code>tools/rgbench</code></div>
      </div>
      <p class="footnote">
        {{ t(
          'Databases ran one at a time in Docker on one machine, each with a warm-up; numbers are indicative, not production absolutes.',
          '各数据库在 Docker 中逐个单独启动并预热后测量。数据用于横向对比，不代表生产服务器上的绝对值。'
        ) }}
      </p>
      <p class="footnote">
        {{ t(
          'SQL Server, MariaDB and Redis are not part of this run.',
          '本次未测试 SQL Server、MariaDB 与 Redis。'
        ) }}
      </p>
    </section>
  </div>
</template>

<style scoped>
.report {
  display: flex;
  flex-direction: column;
  gap: 3rem;
  margin-top: 1rem;
}

.block-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 1rem;
  border-bottom: 1px solid var(--vp-c-border);
  padding-bottom: 0.5rem;
}

.block-head h2 {
  margin: 0;
  font-size: 1.35rem;
  letter-spacing: -0.01em;
}

.muted {
  color: var(--vp-c-text-mute);
  font-size: 0.78rem;
  font-weight: 400;
}

.lede {
  margin: 0.75rem 0 1.25rem;
  color: var(--vp-c-text-2);
  font-size: 0.9rem;
}

.size-grid {
  display: grid;
  grid-template-columns: 170px minmax(0, 1fr);
  gap: 1.5rem;
  align-items: center;
}

.stat {
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  padding: 1rem 1.1rem;
  background: var(--vp-c-bg-alt);
}

.stat-value {
  font-size: 2.4rem;
  font-weight: 650;
  letter-spacing: -0.04em;
  color: var(--vp-c-text-1);
  line-height: 1;
}

.stat-unit {
  margin-left: 0.25rem;
  color: var(--vp-c-text-mute);
  font-size: 0.8rem;
}

.stat-label {
  display: block;
  margin-top: 0.45rem;
  color: var(--vp-c-text-2);
  font-size: 0.76rem;
}

.size-bars,
.metric-bars {
  display: flex;
  flex-direction: column;
  gap: 0.45rem;
}

.bar-row {
  display: grid;
  grid-template-columns: 130px minmax(60px, 1fr) 72px;
  align-items: center;
  gap: 0.6rem;
  font-size: 0.76rem;
}

.bar-label {
  color: var(--vp-c-text-2);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.bar-label.engine {
  display: flex;
  align-items: center;
  gap: 0.35rem;
}

.bar-label em {
  font-style: normal;
  font-size: 0.62rem;
  color: var(--vp-c-accent);
  border: 1px solid var(--vp-c-accent);
  border-radius: 999px;
  padding: 0 0.3rem;
  line-height: 1.3;
}

.bar-track {
  display: block;
  height: 8px;
  background: var(--vp-c-bg-soft);
  border-radius: 999px;
  overflow: hidden;
}

.bar-fill {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: var(--vp-c-accent);
  transition: width 0.25s ease;
}

.bar-value {
  text-align: right;
  color: var(--vp-c-text-1);
  font-variant-numeric: tabular-nums;
  font-size: 0.72rem;
}

.dot {
  display: inline-block;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  flex: none;
}

.dataset-toggle {
  display: flex;
  align-items: center;
  gap: 0.4rem;
  margin-bottom: 1rem;
}

.dataset-toggle button {
  border: 1px solid var(--vp-c-border);
  background: transparent;
  color: var(--vp-c-text-2);
  border-radius: 999px;
  padding: 0.2rem 0.7rem;
  font-size: 0.75rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.dataset-toggle button.active {
  background: var(--vp-c-accent-bg);
  border-color: var(--vp-c-accent-bg);
  color: #fff;
}

.dataset-toggle .rows {
  margin-left: auto;
}

.metric-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 1rem;
}

.metric-card {
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
  padding: 0.8rem 0.9rem;
  background: var(--vp-c-bg);
}

.metric-head {
  display: flex;
  justify-content: space-between;
  margin-bottom: 0.6rem;
}

.metric-name {
  font-size: 0.8rem;
  font-weight: 600;
  color: var(--vp-c-text-1);
}

.metric-hint {
  font-size: 0.68rem;
  color: var(--vp-c-text-mute);
}

.all-ops {
  margin-top: 1.5rem;
  border-top: 1px solid var(--vp-c-border);
  padding-top: 0.8rem;
}

.all-ops summary {
  cursor: pointer;
  font-size: 0.8rem;
  color: var(--vp-c-text-2);
  user-select: none;
}

.table-wrap {
  overflow-x: auto;
  margin-top: 0.7rem;
}

.all-ops table {
  width: 100%;
  font-size: 0.74rem;
  border-collapse: collapse;
}

.all-ops th,
.all-ops td {
  padding: 0.35rem 0.5rem;
  border-bottom: 1px solid var(--vp-c-border);
  text-align: left;
  white-space: nowrap;
}

.all-ops th {
  color: var(--vp-c-text-mute);
  font-weight: 500;
}

.all-ops .num {
  text-align: right;
  font-variant-numeric: tabular-nums;
}

.env {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 0.4rem 1rem;
  margin-top: 1.2rem;
  padding: 0.9rem 1rem;
  border-radius: 10px;
  background: var(--vp-c-bg-soft);
  font-size: 0.72rem;
  color: var(--vp-c-text-2);
}

.runtime {
  display: flex;
  flex-wrap: wrap;
  gap: 0.4rem 1.6rem;
  margin-top: 1rem;
  padding: 0.7rem 0.9rem;
  border: 1px solid var(--vp-c-border);
  border-radius: 10px;
}

.runtime-item {
  display: flex;
  align-items: baseline;
  gap: 0.4rem;
  font-size: 0.74rem;
  color: var(--vp-c-text-2);
}

.runtime-item b {
  color: var(--vp-c-text-1);
  font-size: 0.85rem;
}

.runtime-label {
  color: var(--vp-c-text-mute);
  font-size: 0.68rem;
}

.env b {
  display: block;
  color: var(--vp-c-text-mute);
  font-weight: 500;
  font-size: 0.66rem;
  margin-bottom: 0.1rem;
}

code {
  background: var(--vp-c-bg-alt);
  border-radius: 4px;
  padding: 0.05rem 0.3rem;
  font-size: 0.9em;
}

.footnote {
  margin: 0.8rem 0 0;
  color: var(--vp-c-text-mute);
  font-size: 0.7rem;
  line-height: 1.6;
}

@media (max-width: 860px) {
  .size-grid,
  .metric-grid,
  .env {
    grid-template-columns: minmax(0, 1fr);
  }

  .bar-row {
    grid-template-columns: 105px minmax(40px, 1fr) 62px;
  }
}
</style>
