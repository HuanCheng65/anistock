<script setup lang="ts">
// 热度期货 on a work (docs/design/futures-ui.md §11): the contract as one question, "will these 4 weeks average
// above the price?", its chart (FutChart), the player's position as one sentence, the contract's numbers and
// book, and 押更热 / 押更冷. Desk: the same frame as the stock page, the book and the order form in the right
// column; phone: a bar at the bottom opening the order sheet.
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import FutChart from '../components/futures/FutChart.vue'
import FutOrderForm from '../components/futures/FutOrderForm.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import { DESK, useMedia } from '../composables/useMedia'
import { usePoll } from '../composables/usePoll'
import { useTopic } from '../live'
import { api, call, errorText } from '../api'
import { dir, money, num, pct, qty } from '../format'
import { useFutures } from '../stores/futures'
import { useSession } from '../stores/session'

const route = useRoute()
const router = useRouter()
const fut = useFutures()
const session = useSession()
const desk = useMedia(DESK)
const id = computed(() => Number(route.params.id))
const err = ref('')
const sheet = ref<'buy' | 'sell' | null>(null)

usePoll(async () => {
  try {
    await fut.refresh(id.value)
    err.value = ''
  } catch (e) {
    err.value = errorText(e, '无法加载合约')
  }
}, 5000, 20000)
// live: the contract and its book come pushed; the poll then only picks up the days and the index
useTopic(computed(() => (Number.isFinite(id.value) ? `futures:${id.value}` : null)))
usePoll(() => (session.me ? fut.refreshMine() : null), 15000, 60000)
usePoll(() => (fut.list ? null : fut.refreshList()), 60000)

const d = computed(() => fut.detail[id.value] ?? null)
// the work's heat before the period: what a player has to go on before it begins (the chart's lighter bars)
const workHeat = ref<{ day: string; index: number }[]>([])
async function loadHeat() {
  const k = d.value?.contract
  if (!k) return
  const r = await call(api.GET('/api/v1/stocks/{id}/heat', { params: { path: { id: k.stock.id } } }))
  workHeat.value = (r.works.find((w) => w.work_id === k.work_id)?.days ?? []).map((x) => ({ day: x.day, index: x.index }))
}
watch(() => d.value?.contract.id, () => loadHeat().catch(() => {}), { immediate: true })
usePoll(() => loadHeat().catch(() => {}), 300000)
const c = computed(() => d.value?.contract ?? null)
const today = computed(() => session.status?.trading_day)
const pos = computed(() => (fut.mine?.positions ?? []).find((p) => p.contract.id === id.value) ?? null)
const left = computed(() => (c.value ? c.value.days_total - c.value.days_published : 0))
const span = (a: string, b: string) => `${Number(a.slice(5, 7))}月${Number(a.slice(8))}日–${Number(b.slice(5, 7))}月${Number(b.slice(8))}日`
const price = computed(() => (c.value ? c.value.last_price / 100 : 0))
const before = computed(() => {
  const k = c.value
  if (!k) return []
  return workHeat.value.filter((x) => x.day < k.first_day).slice(desk.value ? -28 : -14)
})
/** the last 7 days of the work's index (shown in place of the period's average before it has one) */
const week = computed(() => {
  const xs = workHeat.value.slice(-7)
  return xs.length ? xs.reduce((a, x) => a + x.index, 0) / xs.length : null
})
/** day by day, newest first: the day's index, the settlement, the trading */
const rows = computed(() => {
  if (!d.value) return []
  const idx = new Map(d.value.index.map((h) => [h.day, h.index]))
  return [...d.value.days].reverse().map((r) => ({ ...r, heat: idx.get(r.day) ?? null }))
})
const SOURCE: Record<string, string> = { vwap: '收盘前 30 分钟均价', mid: '做市报价中间价', prev: '沿用前结算价', final: '到期结算' }
/** the other contract of the same work while two overlap (the old one's last week) */
const sibling = computed(() => (fut.list ?? []).find((x) => c.value && x.stock.id === c.value.stock.id && x.id !== c.value.id && x.status === 'listed') ?? null)
watch(id, () => (sheet.value = null))

function back() {
  if (window.history.state?.back) router.back()
  else router.push('/futures')
}
</script>

<template>
  <main class="page futpage">
    <header class="pagehead back phone-only">
      <button type="button" class="iconbtn" aria-label="返回" @click="back"><Icon name="back" /></button>
      <span class="clamp1 ttl">{{ c?.stock.title }}</span>
    </header>
    <div class="frame">
      <div class="center">
        <EmptyState v-if="err && !d" :text="err" action="返回" to="/futures" tone="error" />
        <template v-else-if="d && c">
          <nav class="kind gutter" aria-label="品种">
            <RouterLink :to="`/stocks/${c.stock.id}`">股票</RouterLink>
            <span aria-current="page">热度期货</span>
          </nav>

          <section class="q gutter">
            <div class="meta">
              <span class="desk-only serif">{{ c.stock.title }}</span>
              <span>第 {{ c.period }} 期 · {{ span(c.first_day, c.last_day) }}<template v-if="c.status === 'listed' && today && c.first_day > today"> · 还没开始</template><template v-else-if="c.status === 'listed' && left"> · 还剩 {{ left }} 天</template></span>
              <RouterLink v-if="sibling" :to="`/futures/${sibling.id}`" class="sib">第 {{ sibling.period }} 期</RouterLink>
            </div>
            <h1 v-if="c.status === 'settled' && c.final_price != null" class="serif">这 4 周的平均热度是 <span class="num">{{ num(c.final_price / 100) }}</span></h1>
            <h1 v-else-if="c.status === 'expiring'" class="serif">等待最后一天的热度公布，按 4 周的平均结算</h1>
            <h1 v-else class="serif">这 4 周的平均热度，会高于 <span class="num up">{{ num(price) }}</span> 吗？</h1>
          </section>

          <section class="chart gutter">
            <FutChart :c="c" :index="d.index" :before="before" :settles="d.days.map((r) => ({ day: r.day, settle: r.settle }))" :today="today" :wide="desk" />
            <dl class="two">
              <div v-if="c.days_published"><dt>已公布 {{ c.days_published }} 天，平均</dt><dd class="num">{{ c.average_so_far != null ? num(c.average_so_far) : '–' }}</dd></div>
              <div v-else><dt>最近 7 天平均</dt><dd class="num">{{ week != null ? num(week) : '–' }}</dd></div>
              <div><dt>市场预期 {{ c.days_total }} 天平均</dt><dd class="num up">{{ num(price) }} <span class="chg" :class="dir(c.change)">今日 {{ pct(c.change, 1) }}</span></dd></div>
            </dl>
          </section>

          <p class="how gutter"><RouterLink to="/rules/heat-index">指数编制方法</RouterLink></p>

          <section v-if="pos && pos.quantity" class="mine gutter">
            <p>押<b :class="pos.quantity > 0 ? 'up' : 'down'">{{ pos.quantity > 0 ? '更热' : '更冷' }}</b> {{ qty(Math.abs(pos.quantity)) }} 手，成本 <span class="num">{{ num(pos.average_price / 100) }}</span>。按现在的预期，{{ pos.pnl >= 0 ? '赚' : '亏' }} <span class="num" :class="dir(pos.pnl)">{{ money(Math.abs(pos.pnl), 0) }}</span> 星辉。</p>
            <p class="sub">初始保证金 <span class="num">{{ money(pos.margin) }}</span></p>
          </section>

          <section class="stats gutter">
            <dl>
              <div><dt>前结算</dt><dd class="num">{{ num(c.prev_settle / 100) }}</dd></div>
              <div><dt>持仓量</dt><dd class="num">{{ qty(c.open_interest) }} 手</dd></div>
              <div><dt>成交</dt><dd class="num">{{ qty(c.volume_today) }} 手</dd></div>
              <div><dt>每手初始保证金</dt><dd class="num">{{ money(c.initial_margin, 0) }}</dd></div>
              <div><dt>每手每点</dt><dd class="num">{{ money(c.tick_value * 100, 0) }}</dd></div>
            </dl>
            <div class="book term phone-only">
              <div><div v-for="(l, i) in d.bids.slice(0, 3)" :key="`b${i}`" class="lv"><span class="k">买{{ i + 1 }}</span><span class="num tup">{{ num(l.price / 100) }}</span><span class="num">{{ l.quantity }}</span></div></div>
              <div><div v-for="(l, i) in d.asks.slice(0, 3)" :key="`a${i}`" class="lv"><span class="k">卖{{ i + 1 }}</span><span class="num tdown">{{ num(l.price / 100) }}</span><span class="num">{{ l.quantity }}</span></div></div>
            </div>
          </section>

          <section v-if="rows.length" class="days gutter" aria-labelledby="days-h">
            <h2 id="days-h" class="h2">每日结算</h2>
            <div class="tw">
              <table class="table">
                <thead><tr><th>日期</th><th>当天热度</th><th>结算价</th><th class="desk-only">取法</th><th>成交</th><th>持仓量</th></tr></thead>
                <tbody>
                  <tr v-for="r in rows" :key="r.day">
                    <td class="num">{{ Number(r.day.slice(5, 7)) }}/{{ Number(r.day.slice(8)) }}</td>
                    <td class="num">{{ r.heat != null ? num(r.heat) : '–' }}</td>
                    <td class="num">{{ num(r.settle / 100) }}</td>
                    <td class="desk-only muted">{{ SOURCE[r.settle_source] ?? r.settle_source }}</td>
                    <td class="num">{{ qty(r.volume) }}</td>
                    <td class="num">{{ qty(r.open_interest) }}</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>
        </template>
        <div v-else class="gutter" style="padding-top: 18px">
          <div class="skel" style="height: 90px"></div>
          <div class="skel" style="height: 200px; margin-top: 16px"></div>
        </div>
      </div>

      <aside v-if="d && c" class="desk-only right term" aria-label="交易">
        <section class="tsec">
          <div class="dbook">
            <div v-for="(l, i) in [...d.asks.slice(0, 5)].reverse()" :key="`a${i}`" class="lv"><span class="k">卖{{ Math.min(5, d.asks.length) - i }}</span><span class="num tdown">{{ num(l.price / 100) }}</span><span class="num">{{ l.quantity }}</span></div>
            <div class="last num">{{ num(price) }}</div>
            <div v-for="(l, i) in d.bids.slice(0, 5)" :key="`b${i}`" class="lv"><span class="k">买{{ i + 1 }}</span><span class="num tup">{{ num(l.price / 100) }}</span><span class="num">{{ l.quantity }}</span></div>
          </div>
        </section>
        <section class="tsec"><FutOrderForm :c="c" :d="d" :pos="pos" /></section>
      </aside>
    </div>

    <template v-if="d && c && c.status === 'listed' && !desk">
      <div class="tradebar">
        <button type="button" class="hot" @click="sheet = 'buy'"><b>押更热</b><span class="num">平均高于 {{ num(price) }} 赚</span></button>
        <button type="button" class="cold" @click="sheet = 'sell'"><b>押更冷</b><span class="num">平均低于 {{ num(price) }} 赚</span></button>
      </div>
      <div v-if="sheet" class="scrim" @click.self="sheet = null">
        <div class="sheet term" role="dialog" aria-label="下单">
          <div class="sheethead">
            <span class="clamp1 serif">{{ c.stock.title }} · 第 {{ c.period }} 期</span>
            <button type="button" class="iconbtn x" aria-label="关闭" @click="sheet = null"><Icon name="close" :size="20" /></button>
          </div>
          <FutOrderForm :c="c" :d="d" :pos="pos" :initial-side="sheet" />
        </div>
      </div>
    </template>
  </main>
</template>

<style scoped>
.ttl { font-size: 15px; font-weight: 700; }
.kind { display: flex; gap: 22px; border-bottom: 1px solid var(--hair); font-size: 15px; }
.kind a, .kind span { padding: 11px 0 9px; color: var(--muted); border-bottom: 2px solid transparent; }
.kind [aria-current] { color: var(--ink); font-weight: 700; border-bottom-color: var(--ink); }
.q { padding-top: 18px; display: flex; flex-direction: column; gap: 6px; }
.meta { display: flex; gap: 10px; align-items: baseline; flex-wrap: wrap; font-size: 13px; color: var(--ink-2); }
.meta .serif { font-weight: 900; color: var(--ink); }
.sib { margin-left: auto; color: var(--accent); }
.q h1 { font-size: 23px; font-weight: 900; line-height: 1.35; }
.up { color: var(--up); }
.down { color: var(--down); }
.chart { padding-top: 12px; display: flex; flex-direction: column; gap: 6px; }
.two { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.two div { display: flex; flex-direction: column; gap: 2px; }
.two dt { font-size: 12px; color: var(--muted); }
.two dd { font-size: 22px; font-weight: 600; }
.chg { font-size: 13px; font-weight: 400; }
.how { font-size: 12px; text-align: right; margin-top: 4px; }
.how a { color: var(--accent); }
.mine { margin-top: 16px; padding-top: 14px; padding-bottom: 14px; background: var(--ground-2); display: flex; flex-direction: column; gap: 4px; }
.mine p { font-size: 15px; line-height: 1.6; }
.mine .sub { font-size: 12px; color: var(--muted); }
.stats { margin-top: 16px; padding-top: 12px; border-top: 1px solid var(--hair); display: flex; flex-direction: column; gap: 12px; }
.stats dl { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); row-gap: 8px; column-gap: 12px; font-size: 12px; }
.stats dl div { display: flex; justify-content: space-between; gap: 6px; }
.stats dt { color: var(--muted); white-space: nowrap; }
.days { margin-top: 18px; border-top: 1px solid var(--rule); padding-top: 12px; display: flex; flex-direction: column; gap: 8px; }
.tw { overflow-x: auto; }
.days td, .days th { text-align: right; }
.days td:first-child, .days th:first-child { text-align: left; }
.muted { color: var(--muted); font-size: 12px; }
.book { padding: 12px; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 14px; font-size: 12px; }
.lv { display: grid; grid-template-columns: 2.4em minmax(0, 1fr) auto; gap: 8px; padding: 2px 0; }
.lv .k { color: var(--t-ink-2); }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.page.futpage { padding-bottom: calc(80px + env(safe-area-inset-bottom, 0px)); }
.tradebar {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 30; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px;
  padding: 10px var(--gutter) calc(10px + env(safe-area-inset-bottom, 0px)); background: var(--ground); border-top: 1px solid var(--rule);
}
.tradebar button { height: 50px; border: 0; color: #fff; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 1px; }
.tradebar b { font-size: 16px; }
.tradebar span { font-size: 11px; }
.tradebar .hot { background: var(--up); }
.tradebar .cold { background: var(--down); }
.scrim { position: fixed; inset: 0; z-index: 60; background: rgba(0, 0, 0, 0.45); display: flex; align-items: flex-end; }
.sheet { width: 100%; max-height: 92vh; overflow: auto; padding: 4px var(--gutter) calc(16px + env(safe-area-inset-bottom, 0px)); animation: sheet-in var(--dur-2) var(--ease-out); }
@keyframes sheet-in { from { transform: translateY(24px); opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .sheet { animation: none; } }
.sheethead { display: flex; align-items: center; gap: 10px; padding: 6px 0 8px; font-size: 15px; }
.sheethead .serif { flex: 1; min-width: 0; font-weight: 600; }
.x { color: var(--t-ink-2); margin-right: -10px; }

@media (min-width: 1024px) {
  .page.futpage { padding-bottom: 0; }
  .frame { display: grid; grid-template-columns: minmax(0, 1fr) 340px; align-items: start; max-width: calc(var(--container) + 64px); margin: 0 auto; padding: 0 32px; }
  .center { --gutter: 28px; min-width: 0; padding-bottom: 48px; }
  .right { position: sticky; top: var(--desk-header-h); max-height: calc(100vh - var(--desk-header-h)); overflow: auto; min-height: calc(100vh - var(--desk-header-h)); }
  .tsec { padding: 12px 16px; border-bottom: 1px solid var(--t-line); }
  .dbook { display: flex; flex-direction: column; font-size: 12px; }
  .dbook .last { font-size: 18px; padding: 6px 0; color: var(--t-ink); border-top: 1px solid var(--t-line); border-bottom: 1px solid var(--t-line); margin: 4px 0; }
  .q h1 { font-size: 32px; }
  .meta { font-size: 14px; }
  .chart { padding-top: 18px; }
  .stats dl { grid-template-columns: repeat(5, minmax(0, 1fr)); }
  .stats dl div { flex-direction: column; gap: 2px; }
  .stats dd { font-size: 14px; }
}
</style>
