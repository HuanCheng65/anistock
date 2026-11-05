<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api, ApiError, type MarketStock } from '../api'
import Sparkline from '../components/Sparkline.vue'
import { usePoll } from '../composables/usePoll'
import { big, dir, episodes, money, pct } from '../format'

const stocks = ref<MarketStock[] | null>(null)
const error = ref('')
const q = ref('')
type SortKey = 'change' | 'turnover' | 'revenue' | 'revchange' | 'code'
const sort = ref<SortKey>('change')
const desc = ref(true)
const filter = ref<'all' | 'cooling' | 'new' | 'airing'>('all')

usePoll(async () => {
  try {
    stocks.value = (await api.get<{ stocks: MarketStock[] }>('/api/market')).stocks
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '行情加载失败'
  }
}, 5000)

const sorters: Record<SortKey, (s: MarketStock) => number | string> = {
  change: (s) => s.change,
  turnover: (s) => s.turnover,
  revenue: (s) => s.week_revenue,
  revchange: (s) => s.revenue_change ?? -Infinity,
  code: (s) => s.code,
}

function setSort(k: SortKey) {
  if (sort.value === k) desc.value = !desc.value
  else {
    sort.value = k
    desc.value = k !== 'code'
  }
}

const rows = computed(() => {
  if (!stocks.value) return []
  const term = q.value.trim().toLowerCase()
  let list = stocks.value.filter((s) =>
    !term || [s.title, s.title_ja, s.title_en].some((t) => (t ?? '').toLowerCase().includes(term)) || s.code.includes(term))
  if (filter.value === 'cooling') list = list.filter((s) => s.cooling)
  if (filter.value === 'new') list = list.filter((s) => s.new)
  if (filter.value === 'airing') list = list.filter((s) => s.aired > 0 && (s.long_runner || !s.episodes || s.aired < s.episodes))
  const f = sorters[sort.value]
  return [...list].sort((a, b) => {
    const x = f(a), y = f(b)
    const c = x < y ? -1 : x > y ? 1 : 0
    return desc.value ? -c : c
  })
})

const breadth = computed(() => {
  const s = stocks.value ?? []
  return {
    up: s.filter((x) => x.change > 1e-9).length,
    down: s.filter((x) => x.change < -1e-9).length,
    flat: s.filter((x) => Math.abs(x.change) <= 1e-9).length,
    turnover: s.reduce((a, x) => a + x.turnover, 0),
    cooling: s.filter((x) => x.cooling).length,
  }
})

const sortLabels: [SortKey, string][] = [['change', '涨跌幅'], ['turnover', '成交额'], ['revenue', '本周营收'], ['revchange', '营收增速'], ['code', '代码']]
</script>

<template>
  <main class="page">
    <section class="panel summary" v-if="stocks">
      <div><span class="label">上涨</span><span class="num up">{{ breadth.up }}</span></div>
      <div><span class="label">下跌</span><span class="num down">{{ breadth.down }}</span></div>
      <div><span class="label">平盘</span><span class="num">{{ breadth.flat }}</span></div>
      <div><span class="label">今日成交额</span><span class="num">{{ big(breadth.turnover) }}</span></div>
      <RouterLink to="/ipo" class="ipo-call"><span class="tag ipo">新股</span> 认购与配售 →</RouterLink>
    </section>

    <section class="controls">
      <input id="market-search" class="input" v-model="q" placeholder="搜索番名（中文 / 日文 / 英文）或代码" type="search" autocomplete="off" />
      <div class="chips" role="group" aria-label="筛选">
        <button v-for="[k, l] in ([['all', '全部'], ['airing', '播出中'], ['new', '新上市'], ['cooling', '冷静期']] as const)" :key="k"
                type="button" class="chip" :aria-pressed="filter === k" @click="filter = k">{{ l }}</button>
      </div>
      <div class="chips" role="group" aria-label="排序">
        <button v-for="[k, l] in sortLabels" :key="k" type="button" class="chip" :aria-pressed="sort === k" @click="setSort(k)">
          {{ l }}<span v-if="sort === k" aria-hidden="true">{{ desc ? ' ↓' : ' ↑' }}</span>
        </button>
      </div>
    </section>

    <p v-if="error" class="notice error">{{ error }}</p>

    <section class="panel list" v-if="stocks">
      <div class="head small muted">
        <span>名称 / 代码</span><span class="hide-sm">近 20 日</span><span class="hide-sm r">本周营收</span><span class="r">现价</span><span class="r">涨跌幅</span>
      </div>
      <RouterLink v-for="s in rows" :key="s.id" :to="`/stock/${s.id}`" class="row">
        <div class="name">
          <span class="title">{{ s.title }}</span>
          <span v-if="s.title_ja && s.title_ja !== s.title" class="sub small muted">{{ s.title_ja }}</span>
          <span class="meta small">
            <span class="num muted">{{ s.code }}</span>
            <span v-if="s.cooling" class="tag new">冷静期</span>
            <span v-if="s.new" class="tag ipo">新上市</span>
            <span class="muted">{{ episodes(s.aired, s.episodes, s.long_runner) }}</span>
          </span>
        </div>
        <Sparkline class="hide-sm" :values="s.spark" :base="s.spark[0]" />
        <div class="hide-sm r rev">
          <span class="num">{{ big(s.week_revenue) }}</span>
          <span class="num small" :class="dir(s.revenue_change)">{{ s.revenue_change == null ? '–' : pct(s.revenue_change, 0) }}</span>
        </div>
        <div class="r px num" :class="dir(s.change)">{{ money(s.price) }}</div>
        <div class="r"><span class="chg num" :class="dir(s.change)">{{ pct(s.change) }}</span></div>
      </RouterLink>
      <p v-if="!rows.length" class="empty">没有符合条件的股票</p>
    </section>

    <section v-else-if="!error" class="panel list">
      <div v-for="i in 8" :key="i" class="row"><div class="skeleton" style="height: 34px; grid-column: 1 / -1"></div></div>
    </section>
  </main>
</template>

<style scoped>
.summary { display: flex; flex-wrap: wrap; gap: 6px 20px; padding: 10px 12px; align-items: baseline; }
.summary > div { display: flex; gap: 6px; align-items: baseline; }
.summary .num { font-size: 16px; font-weight: 600; }
.ipo-call { margin-left: auto; background: none; border: none; cursor: pointer; font-size: 13px; color: var(--ink-2); display: flex; gap: 6px; align-items: center; padding: 0; }
.controls { display: flex; flex-direction: column; gap: 8px; }
.chips { display: flex; gap: 6px; overflow-x: auto; scrollbar-width: none; }
.chip {
  flex: none; height: 30px; padding: 0 12px; border-radius: 15px; border: 1px solid var(--rule-strong);
  background: var(--surface); font-size: 13px; cursor: pointer; color: var(--ink-2);
}
.chip[aria-pressed="true"] { background: var(--ink); color: var(--surface); border-color: var(--ink); }

.list { overflow: hidden; }
.head, .row {
  display: grid; align-items: center; gap: 10px; padding: 10px 12px;
  grid-template-columns: minmax(0, 1fr) auto auto;
}
.head { padding-block: 8px; border-bottom: 1px solid var(--rule); }
.row { border-bottom: 1px solid var(--rule); color: var(--ink); }
.row:last-child { border-bottom: none; }
.row:hover { background: var(--surface-2); }
.r { text-align: right; }
.name { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.title { font-weight: 500; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.sub { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; margin-top: -2px; }
.meta { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; }
.px { font-size: 15px; font-weight: 600; min-width: 4.5em; }
.chg { display: inline-block; min-width: 5.4em; text-align: center; padding: 4px 6px; border-radius: 4px; font-weight: 600; font-size: 13px; }
.chg.up { background: var(--up); color: #fff; }
.chg.down { background: var(--down); color: #fff; }
.chg.flat { background: var(--surface-2); color: var(--ink-2); }
.rev { display: flex; flex-direction: column; line-height: 1.3; }
.hide-sm { display: none; }
@media (min-width: 720px) {
  .head, .row { grid-template-columns: minmax(0, 1fr) 70px 90px auto auto; }
  .hide-sm { display: block; }
  .rev.hide-sm { display: flex; }
}
</style>
