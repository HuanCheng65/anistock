<script setup lang="ts">
// 股票: every listed stock in one sortable, filterable table (table frame).
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import type { MarketStock } from '../api'
import ChangePill from '../components/ChangePill.vue'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import SearchPanel from '../components/SearchPanel.vue'
import Sparkline from '../components/Sparkline.vue'
import { usePoll } from '../composables/usePoll'
import { FILTERS, listRows, SORTS, sortValue, type ListCtx, type SortKey } from '../composables/stockList'
import { big, dir, episodeText, money, pct, share } from '../format'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'

const market = useMarket()
const prefs = usePrefs()
const route = useRoute()
const router = useRouter()
const searching = ref(false)

const { refresh } = usePoll(() => market.refreshQuotes(), 5000)
usePoll(() => market.refreshRatios(), 60000)
usePoll(() => (market.indices ? null : market.refreshIndices()), 300000)

const q = (k: string) => (typeof route.query[k] === 'string' ? (route.query[k] as string) : '')
const filter = ref(q('f') || (prefs.watchlist.length ? 'all' : 'all'))
const sort = ref<SortKey>((SORTS.find((x) => x.key === q('s'))?.key ?? 'change') as SortKey)
const desc = ref(q('d') !== 'asc')
watch([filter, sort, desc], () => router.replace({ query: { f: filter.value, s: sort.value, d: desc.value ? 'desc' : 'asc' } }))

const ctx = computed<ListCtx>(() => ({
  watch: new Set(prefs.watchlist),
  seasonStart: market.indices?.season_start ?? market.ratios?.season_start ?? '9999',
  ratio: market.ratioById,
}))
const counts = computed(() => Object.fromEntries(FILTERS.map((f) => [f.key, (market.stocks ?? []).filter((s) => f.test(s, ctx.value)).length])))
const rows = computed(() => listRows(market.stocks ?? [], filter.value, sort.value, desc.value, ctx.value))
const filterLabel = computed(() => FILTERS.find((f) => f.key === filter.value)?.label ?? '全部')

function setSort(k: SortKey) {
  if (sort.value === k) desc.value = !desc.value
  else {
    sort.value = k
    desc.value = true
  }
}
const ariaSort = (k: SortKey) => (sort.value === k ? (desc.value ? 'descending' : 'ascending') : 'none')
const arrow = (k: SortKey) => (sort.value === k ? (desc.value ? ' ↓' : ' ↑') : '')

// phone: the right column shows one metric; its header cycles through them
const PHONE_METRICS: SortKey[] = ['change', 'turnover', 'cap', 'pe', 'dy', 'rev']
const metric = computed<SortKey>(() => (PHONE_METRICS.includes(sort.value) ? sort.value : 'change'))
function nextMetric() {
  const i = PHONE_METRICS.indexOf(metric.value)
  sort.value = PHONE_METRICS[(i + 1) % PHONE_METRICS.length]
  desc.value = true
}
function fmt(s: MarketStock, k: SortKey): string {
  const v = sortValue(s, k, ctx.value)
  if (v == null) return '–'
  switch (k) {
    case 'price': return money(v)
    case 'change': return pct(v)
    case 'turnover': case 'cap': case 'rev': return big(v)
    case 'pe': return v.toFixed(1)
    case 'dy': return share(v, 1)
    case 'eps': return s.aired ? episodeText(s.aired, s.episodes, s.long_runner) : '–'
  }
}
const label = (k: SortKey) => SORTS.find((x) => x.key === k)!.label

function remember() {
  prefs.context = { key: filter.value, label: filterLabel.value, ids: rows.value.map((s) => s.id) }
}
</script>

<template>
  <main class="page stockspage">
    <header class="pagehead phone-only">
      <h1>股票</h1>
      <button type="button" class="iconbtn sbtn" aria-label="搜索" @click="searching = true"><Icon name="search" :size="20" /></button>
    </header>
    <div v-if="searching" class="searchsheet phone-only">
      <div class="sbar">
        <SearchPanel autofocus class="sp" @close="searching = false" />
        <button type="button" class="linkbtn cancel" @click="searching = false">取消</button>
      </div>
    </div>

    <div class="container">
      <h1 class="pagetitle desk-only">股票</h1>
      <div class="chips" role="group" aria-label="筛选">
        <button v-for="f in FILTERS" :key="f.key" type="button" :aria-pressed="filter === f.key" @click="filter = f.key">
          {{ f.label }}<span v-if="market.stocks" class="num cnt"> {{ counts[f.key] }}</span>
        </button>
      </div>

      <EmptyState v-if="market.error && !market.stocks" :text="market.error" action="重试" tone="error" @act="refresh" />
      <template v-else-if="market.stocks">
        <!-- phone list -->
        <div class="phone-only plist">
          <div class="phead">
            <span class="muted">{{ rows.length }} 支</span>
            <button type="button" class="sorth" :aria-sort="ariaSort('price')" @click="setSort('price')">现价{{ arrow('price') }}</button>
            <span class="mh">
              <button type="button" class="sorth" :aria-sort="ariaSort(metric)" :aria-label="`${label(metric)}，切换指标`" @click="nextMetric">{{ label(metric) }}</button>
              <button type="button" class="sorth arr" :aria-label="desc ? '改为升序' : '改为降序'" @click="setSort(metric)">{{ sort === metric ? (desc ? '↓' : '↑') : '↕' }}</button>
            </span>
          </div>
          <RouterLink v-for="s in rows" :key="s.id" :to="`/stocks/${s.id}`" class="prow" @click="remember">
            <Cover :id="s.id" :title="s.title" :w="30" :h="42" :spine="false" />
            <span class="nm">
              <span class="serif clamp1 t">{{ s.title }}</span>
              <span class="num code">{{ s.code }}<span v-if="s.cooling" class="tag">冷静期</span><span v-else-if="s.new" class="tag">新上市</span></span>
            </span>
            <span class="num px">{{ money(s.price) }}</span>
            <ChangePill v-if="metric === 'change'" :value="s.change" />
            <span v-else class="num mv">{{ fmt(s, metric) }}</span>
          </RouterLink>
        </div>

        <!-- desktop table -->
        <table class="table dt desk-only">
          <thead>
            <tr>
              <th>名称</th>
              <th v-for="c in SORTS" :key="c.key" :aria-sort="ariaSort(c.key)">
                <button type="button" class="sorth" :aria-sort="ariaSort(c.key)" @click="setSort(c.key)">{{ c.label }}{{ arrow(c.key) }}</button>
              </th>
              <th>近 20 日</th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="s in rows" :key="s.id">
              <td class="namecell">
                <RouterLink :to="`/stocks/${s.id}`" class="nm2" @click="remember">
                  <Cover :id="s.id" :title="s.title" :w="24" :h="34" :spine="false" />
                  <span class="serif clamp1 t">{{ s.title }}</span>
                  <span class="num code">{{ s.code }}</span>
                  <span v-if="s.cooling" class="tag">冷静期</span><span v-else-if="s.new" class="tag">新上市</span>
                </RouterLink>
              </td>
              <td class="num">{{ money(s.price) }}</td>
              <td class="num" :class="dir(s.change)">{{ pct(s.change) }}</td>
              <td class="num">{{ big(s.turnover) }}</td>
              <td class="num">{{ big(s.market_cap) }}</td>
              <td class="num">{{ fmt(s, 'pe') }}</td>
              <td class="num">{{ fmt(s, 'dy') }}</td>
              <td class="num">{{ big(s.week_revenue) }}</td>
              <td class="num">{{ fmt(s, 'eps') }}</td>
              <td><Sparkline :values="s.spark" :base="s.spark[0]" :w="72" :h="22" class="mini" /></td>
            </tr>
          </tbody>
        </table>
        <EmptyState v-if="!rows.length" :text="filter === 'watch' ? '暂无自选' : '暂无符合条件的股票'" :action="filter === 'watch' ? '查看全部' : undefined" @act="filter = 'all'" />
      </template>
      <div v-else><div v-for="i in 8" :key="i" class="skel" style="height: 40px; margin: 12px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.sbtn { margin-left: auto; width: 36px; height: 36px; }
.searchsheet { position: fixed; inset: 0; z-index: 50; background: var(--ground); overflow: auto; }
.sbar { display: flex; gap: 12px; align-items: flex-start; padding: calc(12px + env(safe-area-inset-top, 0px)) 16px; }
.sp { flex: 1; min-width: 0; }
.cancel { color: var(--ink); font-size: 15px; height: 40px; }
.chips { display: flex; gap: 8px; padding: 12px 0; overflow-x: auto; scrollbar-width: none; margin: 0 calc(-1 * var(--side)); padding-left: var(--side); padding-right: var(--side); }
.chips button { flex: none; height: 32px; padding: 0 12px; font-size: 13px; border: 1px solid var(--base-line); background: transparent; color: var(--ink); }
.chips button[aria-pressed="true"] { background: var(--ink); color: var(--ground); border-color: var(--ink); }
.cnt { font-size: 12px; opacity: 0.75; margin-left: 4px; }
.tag { margin-left: 8px; font-family: var(--font); font-size: 11px; color: var(--brand); border: 1px solid currentColor; padding: 0 4px; }

.phead { display: grid; grid-template-columns: minmax(0, 1fr) auto 84px; gap: 12px; align-items: baseline; padding: 6px 0; font-size: 12px; color: var(--muted); border-bottom: 1px solid var(--rule); }
.phead .mh { display: flex; justify-content: flex-end; gap: 4px; }
.arr { min-width: 14px; }
.prow { display: grid; grid-template-columns: 30px minmax(0, 1fr) auto 84px; gap: 12px; align-items: center; padding: 10px 0; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.prow > :first-child { grid-column: 1; }
.phead > :first-child { padding-left: 42px; }
.nm { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.t { font-size: 15px; font-weight: 600; }
.code { font-size: 11px; color: var(--muted); }
.px { font-size: 15px; text-align: right; }
.mv { font-size: 14px; text-align: right; }
.prow :deep(.pill) { min-width: 0; width: 84px; }

.dt th { padding-top: 10px; padding-bottom: 8px; border-bottom: 1px solid var(--rule); }
.dt td { padding-top: 9px; padding-bottom: 9px; font-size: 14px; }
.namecell { max-width: 0; width: 34%; }
.nm2 { display: flex; align-items: center; gap: 10px; min-width: 0; color: var(--ink); }
.nm2 .t { min-width: 0; font-size: 14px; }
.nm2 .code { flex: none; }
.mini { margin-left: auto; }
@media (min-width: 1024px) { .chips { margin: 0; padding: 0 0 8px; flex-wrap: wrap; } .chips button { height: 32px; } }
</style>
