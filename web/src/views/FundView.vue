<script setup lang="ts">
// One index fund: its net value and 今日估算, the net value against the tracked index, size / members / cash, the
// player's units, the 10 largest holdings, the rules, and 申购 / 赎回 (FundSheet).
import { computed, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { api, call, errorText, type MarketIndex } from '../api'
import EmptyState from '../components/EmptyState.vue'
import FundSheet from '../components/FundSheet.vue'
import Icon from '../components/Icon.vue'
import { usePoll } from '../composables/usePoll'
import { bigMoney, dir, money, num, pct, share, signed } from '../format'
import { useFunds } from '../stores/funds'
import { useSession } from '../stores/session'

const route = useRoute()
const router = useRouter()
const funds = useFunds()
const session = useSession()
const key = computed(() => String(route.params.key))
const err = ref('')
const sheet = ref<'buy' | 'sell' | null>(null)

usePoll(async () => {
  try {
    await funds.refresh(key.value)
    err.value = ''
  } catch (e) {
    err.value = errorText(e, '无法加载基金')
  }
}, 15000, 60000)
usePoll(() => (session.me ? funds.refreshMine().catch(() => {}) : null), 15000, 60000)
// the tracked index (total return, as the net value counts the dividends)
const index = ref<MarketIndex | null>(null)
usePoll(async () => {
  const r = await call(api.GET('/api/v1/market/indices', { params: { query: { set: 'all' } } }))
  index.value = r.items.find((i) => i.key === key.value) ?? null
}, 300000)

const d = computed(() => funds.detail[key.value] ?? null)
const f = computed(() => d.value?.fund ?? null)
const hold = computed(() => funds.mine?.holdings.find((h) => h.fund.key === key.value) ?? null)
const md = (x: string | null | undefined) => (x ? `${x.slice(5, 7)}-${x.slice(8)}` : '')

const RANGES = [
  { n: 22, label: '近一月' },
  { n: 66, label: '近三月' },
  { n: 0, label: '成立以来' },
]
const range = ref(0)
const chart = computed(() => {
  const h = d.value?.history ?? []
  if (h.length < 2) return null
  const n = RANGES[range.value].n
  const navs = n ? h.slice(-n - 1) : h
  const idx = new Map((index.value?.series ?? []).map((p) => [p.trading_day, p.value]))
  const days = navs.map((x) => x.day)
  const i0 = days.map((x) => idx.get(x)).find((v) => v != null) ?? null
  const nav = navs.map((x) => x.nav / navs[0].nav)
  const ind = i0 ? days.map((x) => (idx.get(x) ?? NaN) / i0) : []
  const all = [...nav, ...ind.filter((v) => !Number.isNaN(v))]
  const lo = Math.min(...all), hi = Math.max(...all)
  const W = 358, H = 160
  const y = (v: number) => (H - 5 - ((v - lo) / (hi - lo || 1)) * (H - 10)).toFixed(1)
  const pts = (xs: number[]) =>
    xs.map((v, i) => (Number.isNaN(v) ? null : `${((i / (xs.length - 1)) * W).toFixed(1)},${y(v)}`)).filter(Boolean).join(' ')
  const lastIdx = [...ind].reverse().find((v) => !Number.isNaN(v))
  return {
    W, H,
    nav: pts(nav),
    index: pts(ind),
    base: y(1),
    navRet: nav[nav.length - 1] - 1,
    idxRet: lastIdx != null ? lastIdx - 1 : null,
  }
})
const top = computed(() => (d.value?.holdings ?? []).reduce((a, h) => a + h.weight, 0))
const barMax = computed(() => f.value?.cap ?? Math.max(0.0001, ...(d.value?.holdings ?? []).map((h) => h.weight)))

const RULE: Record<string, string> = {
  all: '全部上市股票',
  top20: '流通市值最大的 20 只股票，每月第一个交易日调整',
  long: '长篇连载作品的股票',
  manga: '原作为漫画的股票',
  light_novel: '原作为轻小说的股票',
  isekai: '异世界题材作品的股票',
  romance: '恋爱题材作品的股票',
  action: '动作题材作品的股票',
}
const tracks = computed(() => {
  const x = f.value
  if (!x) return ''
  return `${x.index_name}：${RULE[x.key] ?? ''}，按流通市值加权${x.cap ? `，单只最多占 ${share(x.cap)}` : ''}。`
})

function open(side: 'buy' | 'sell') {
  if (!session.me) router.push({ path: '/login', query: { next: route.fullPath } })
  else sheet.value = side
}
function back() {
  if (window.history.state?.back) router.back()
  else router.push('/funds')
}
</script>

<template>
  <main class="page fundpage">
    <header class="pagehead back phone-only">
      <button type="button" class="iconbtn" aria-label="返回" @click="back"><Icon name="back" /></button>
      <span class="clamp1 ttl">{{ f ? `${f.index_name}基金` : '' }}</span>
    </header>
    <div class="container">
      <EmptyState v-if="err && !d" :text="err" action="返回" to="/funds" tone="error" />
      <template v-else-if="d && f">
        <h1 class="pagetitle desk-only">{{ f.index_name }}基金</h1>
        <section class="nav">
          <div class="lbls"><span>单位净值<template v-if="f.nav_day"> · {{ md(f.nav_day) }}</template></span><span>今日估算</span></div>
          <div class="vals">
            <span class="num big">{{ num(f.nav, 4) }}</span>
            <span class="num est" :class="dir(f.estimate)">{{ pct(f.estimate) }}</span>
          </div>
          <p class="note">今日估算按{{ f.index_name }}的实时点位推算。净值每天收盘后公布。</p>
        </section>

        <section class="chart">
          <div class="ranges" role="group" aria-label="区间">
            <button v-for="(r, i) in RANGES" :key="r.label" type="button" :aria-pressed="range === i" @click="range = i">{{ r.label }}</button>
          </div>
          <template v-if="chart">
            <svg :viewBox="`0 0 ${chart.W} ${chart.H}`" preserveAspectRatio="none" role="img" aria-label="基金净值与指数的走势">
              <line x1="0" :y1="chart.base" :x2="chart.W" :y2="chart.base" class="base" />
              <polyline v-if="chart.index" :points="chart.index" class="idx" />
              <polyline :points="chart.nav" class="fund" />
            </svg>
            <div class="legend">
              <span><i class="k fund"></i>基金 <span class="num" :class="dir(chart.navRet)">{{ pct(chart.navRet) }}</span></span>
              <span v-if="chart.idxRet != null"><i class="k idx"></i>{{ f.index_name }} <span class="num" :class="dir(chart.idxRet)">{{ pct(chart.idxRet) }}</span></span>
            </div>
          </template>
          <p v-else class="note">第一次收盘后显示走势</p>
        </section>

        <dl class="facts">
          <div><dt>规模</dt><dd class="num">{{ bigMoney(f.aum) }}</dd></div>
          <div><dt>成分股</dt><dd class="num">{{ f.members }}</dd></div>
          <div><dt>现金</dt><dd class="num">{{ share(d.cash_weight, 2) }}</dd></div>
        </dl>

        <section v-if="hold" class="mine">
          <span class="ml">
            <span class="muted">持有</span>
            <span><span class="num">{{ money(hold.units) }}</span> 份 · 市值 <span class="num">{{ money(hold.value) }}</span></span>
          </span>
          <span class="num pnl" :class="dir(hold.value - hold.cost)">{{ signed(hold.value - hold.cost) }}</span>
        </section>

        <section v-if="d.holdings.length" class="hold">
          <div class="hh"><h2>持仓</h2><span class="muted">前 {{ d.holdings.length }} 只 · <span class="num">{{ share(top, 2) }}</span></span></div>
          <ul>
            <li v-for="h in d.holdings" :key="h.stock.id">
              <RouterLink :to="`/stocks/${h.stock.id}`">
                <span class="serif clamp1">{{ h.stock.title }}</span>
                <span class="bar"><span :style="{ width: `${Math.min(100, (h.weight / barMax) * 100).toFixed(0)}%` }"></span></span>
                <span class="num w">{{ share(h.weight, 2) }}</span>
              </RouterLink>
            </li>
          </ul>
        </section>

        <section class="rules">
          <h2>规则</h2>
          <dl>
            <dt>跟踪</dt><dd>{{ tracks }}</dd>
            <dt>申购</dt><dd>按金额，至少 {{ money(d.terms.min_subscribe, 0) }} 星辉。提交后的下一个交易时段成交，收盘后按当天的净值确认份额。</dd>
            <dt>赎回</dt><dd>按份额。提交后的下一个交易时段成交，收盘后按当天的净值确认金额。成交前可以撤单。</dd>
            <dt>费用</dt><dd>管理费每年 {{ share(d.terms.fee_rate, 1) }}，每天从净值中扣除。不收申购费。份额持有不满 {{ d.terms.redeem_fee_days }} 天，赎回费 {{ share(d.terms.redeem_fee, 1) }}，归入基金。基金当天的买卖成本由当天申购、赎回的人承担。</dd>
            <dt>分红</dt><dd>基金收到的分红不缴股息税，计入净值。</dd>
          </dl>
        </section>

        <div class="bar-actions">
          <button type="button" class="btn ghost" :disabled="!!session.me && !hold" @click="open('sell')">赎回</button>
          <button type="button" class="btn" @click="open('buy')">申购</button>
        </div>
        <FundSheet v-if="sheet" :open="!!sheet" :fund="f" :side="sheet" :min-subscribe="d.terms.min_subscribe" :redeem-fee="d.terms.redeem_fee"
                   :fee-days="d.terms.redeem_fee_days" @close="sheet = null" />
      </template>
      <div v-else><div v-for="i in 5" :key="i" class="skel" style="height: 60px; margin: 12px 0"></div></div>
    </div>
  </main>
</template>

<style scoped>
.container { max-width: 720px; padding-bottom: calc(80px + env(safe-area-inset-bottom, 0px)); }
section { padding: 16px 0; border-bottom: 1px solid var(--rule); }
.lbls { display: flex; justify-content: space-between; font-size: 13px; color: var(--muted); }
.vals { display: flex; justify-content: space-between; align-items: baseline; margin-top: 4px; }
.big { font-size: 32px; font-weight: 600; }
.est { font-size: 18px; font-weight: 600; }
.note { margin: 8px 0 0; font-size: 12px; line-height: 1.6; color: var(--muted); }
.ranges { display: flex; gap: 8px; margin-bottom: 12px; }
.ranges button { height: 30px; padding: 0 12px; font-size: 13px; border: 1px solid var(--base-line); background: none; color: var(--ink); }
.ranges button[aria-pressed="true"] { background: var(--ink); color: var(--ground); border-color: var(--ink); }
.chart svg { width: 100%; height: 160px; display: block; }
.base { stroke: var(--rule); stroke-dasharray: 3 3; }
.idx { fill: none; stroke: var(--muted); stroke-width: 1.5; vector-effect: non-scaling-stroke; }
.fund { fill: none; stroke: var(--ink); stroke-width: 2; vector-effect: non-scaling-stroke; }
.legend { display: flex; flex-wrap: wrap; gap: 16px; margin-top: 8px; font-size: 13px; }
.k { display: inline-block; width: 14px; height: 2px; vertical-align: middle; margin-right: 6px; }
.k.fund { background: var(--ink); }
.k.idx { background: var(--muted); }
.facts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); margin: 0; padding: 14px 0; border-bottom: 1px solid var(--rule); }
.facts div { display: flex; flex-direction: column; gap: 2px; }
.facts dt { font-size: 12px; color: var(--muted); }
.facts dd { margin: 0; font-size: 16px; }
.mine { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.ml { display: flex; flex-direction: column; gap: 2px; font-size: 14px; }
.pnl { font-size: 17px; font-weight: 600; }
.hh { display: flex; justify-content: space-between; align-items: baseline; }
h2 { margin: 0 0 8px; font-size: 16px; }
.hold ul { list-style: none; margin: 0; padding: 0; }
.hold a { display: grid; grid-template-columns: minmax(0, 1fr) 80px 56px; gap: 10px; align-items: center; padding: 8px 0; color: var(--ink); font-size: 14px; }
.bar { position: relative; height: 6px; background: var(--hair-2); }
.bar span { position: absolute; left: 0; top: 0; bottom: 0; background: var(--ink); }
.w { text-align: right; font-size: 13px; }
.rules dl { display: grid; grid-template-columns: 40px minmax(0, 1fr); gap: 8px 12px; margin: 0; font-size: 13px; line-height: 1.6; }
.rules dt { color: var(--muted); }
.rules dd { margin: 0; }
.bar-actions { position: fixed; left: 0; right: 0; bottom: 0; display: grid; grid-template-columns: 1fr 1fr; gap: 10px; padding: 10px 16px calc(10px + env(safe-area-inset-bottom, 0px)); background: var(--ground); border-top: 1px solid var(--rule); z-index: 5; }
@media (min-width: 1024px) {
  .bar-actions { position: static; padding: 16px 0; border: 0; max-width: 360px; }
  .container { padding-bottom: 24px; }
}
</style>
