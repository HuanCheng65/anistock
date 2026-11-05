<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { api, ApiError, type LedgerEntry, type Order, type Position, type Trade } from '../api'
import { usePoll } from '../composables/usePoll'
import { dir, LEDGER_REASON, money, pct, signed, time } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
const positions = ref<Position[] | null>(null)
const orders = ref<Order[]>([])
const trades = ref<Trade[]>([])
const ledger = ref<LedgerEntry[]>([])
const tab = ref<'orders' | 'trades' | 'ledger'>('orders')
const error = ref('')
const cancelling = ref<number | null>(null)

async function load() {
  try {
    const [p, o] = await Promise.all([
      api.get<{ positions: Position[] }>('/api/portfolio'),
      api.get<{ orders: Order[] }>('/api/orders?status=open'),
      session.loadMe(),
    ])
    positions.value = p.positions
    orders.value = o.orders
    if (tab.value === 'trades') trades.value = (await api.get<{ trades: Trade[] }>('/api/trades')).trades
    if (tab.value === 'ledger') ledger.value = (await api.get<{ entries: LedgerEntry[] }>('/api/ledger')).entries
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '加载失败'
  }
}
const { refresh } = usePoll(load, 5000)

function switchTab(t: typeof tab.value) {
  tab.value = t
  refresh()
}

async function cancel(id: number) {
  cancelling.value = id
  try {
    await api.del(`/api/orders/${id}`)
    await refresh()
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '撤单失败'
  } finally {
    cancelling.value = null
  }
}

const me = computed(() => session.me)
const totals = computed(() => {
  const ps = positions.value ?? []
  return {
    pnl: ps.reduce((a, p) => a + p.pnl, 0),
    divs: ps.reduce((a, p) => a + p.dividends, 0),
  }
})
</script>

<template>
  <main class="page">
    <p v-if="error" class="notice error">{{ error }}</p>

    <section class="panel hero" v-if="me">
      <div class="top">
        <span class="label">总资产（币）</span>
        <span class="total num">{{ money(me.total) }}</span>
        <span class="num" :class="dir(me.return_pct)">较初始 {{ money(me.initial, 0) }}：{{ pct(me.return_pct) }}</span>
      </div>
      <dl class="stats small">
        <div><dt>现金</dt><dd class="num">{{ money(me.cash) }}</dd></div>
        <div><dt>可用</dt><dd class="num">{{ money(me.available) }}</dd></div>
        <div><dt>委托冻结</dt><dd class="num">{{ money(me.frozen) }}</dd></div>
        <div><dt>持仓市值</dt><dd class="num">{{ money(me.stock_value) }}</dd></div>
        <div><dt>累计分红</dt><dd class="num up">{{ money(me.dividends) }}</dd></div>
        <div><dt>存款年利率</dt><dd class="num">{{ (me.deposit_rate * 100).toFixed(2) }}%</dd></div>
      </dl>
      <p class="small muted">现金每天按存款利率自动计息。分红在每周日日结时到账。</p>
    </section>

    <section class="panel">
      <div class="panel-head">
        <h2>持仓</h2>
        <span class="small muted" v-if="positions?.length">浮动 <span class="num" :class="dir(totals.pnl)">{{ signed(totals.pnl) }}</span> · 分红 <span class="num up">{{ money(totals.divs) }}</span></span>
      </div>
      <div v-if="positions && positions.length" class="positions">
        <RouterLink v-for="p in positions" :key="p.company_id" :to="`/stock/${p.company_id}`" class="pos">
          <div class="pname">
            <b>{{ p.title }}</b>
            <span class="small muted num">{{ p.code }} · {{ p.qty }} 股<template v-if="p.available < p.qty">（{{ p.qty - p.available }} 股挂单中）</template></span>
          </div>
          <dl class="pgrid small">
            <div><dt>成本 / 现价</dt><dd class="num">{{ money(p.avg_cost) }} / <span :class="dir(p.day_change)">{{ money(p.price) }}</span></dd></div>
            <div><dt>市值</dt><dd class="num">{{ money(p.value) }}</dd></div>
            <div><dt>浮动盈亏</dt><dd class="num" :class="dir(p.pnl)">{{ signed(p.pnl) }}（{{ pct(p.pnl_pct) }}）</dd></div>
            <div><dt>累计分红</dt><dd class="num up">{{ money(p.dividends) }}</dd></div>
            <div class="total-return"><dt>含分红总盈亏</dt><dd class="num" :class="dir(p.pnl + p.dividends)">{{ signed(p.pnl + p.dividends) }}</dd></div>
          </dl>
        </RouterLink>
        <p class="small muted note">番剧公司把利润按周分给股东，季番的股价会随着集数推进、价值被分走而逐步走低。看盈亏要看“含分红总盈亏”，不要只看股价。</p>
      </div>
      <div v-else-if="positions" class="empty">
        还没有持仓。去 <RouterLink to="/">行情</RouterLink> 挑一部看好的番吧，或者先读 <RouterLink to="/guide">新手指南</RouterLink>。
      </div>
      <div v-else class="panel-body"><div class="skeleton" style="height: 60px"></div></div>
    </section>

    <section class="panel">
      <div class="panel-head tabs">
        <div class="seg" role="tablist">
          <button type="button" role="tab" :aria-pressed="tab === 'orders'" @click="switchTab('orders')">当前委托 {{ orders.length || '' }}</button>
          <button type="button" role="tab" :aria-pressed="tab === 'trades'" @click="switchTab('trades')">成交记录</button>
          <button type="button" role="tab" :aria-pressed="tab === 'ledger'" @click="switchTab('ledger')">资金流水</button>
        </div>
      </div>

      <div v-if="tab === 'orders'">
        <ul v-if="orders.length" class="list">
          <li v-for="o in orders" :key="o.id">
            <RouterLink :to="`/stock/${o.company_id}`" class="lt">{{ o.title }}</RouterLink>
            <span :class="o.side === 'buy' ? 'up' : 'down'">{{ o.side === 'buy' ? '买入' : '卖出' }}</span>
            <span class="num">{{ money(o.price) }} × {{ o.qty }}</span>
            <span class="num muted small">已成 {{ o.filled }}</span>
            <button type="button" class="linkbtn" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button>
          </li>
        </ul>
        <p v-else class="empty">没有挂着的委托</p>
      </div>

      <div v-else-if="tab === 'trades'" class="scroll-x">
        <table class="grid" v-if="trades.length">
          <thead><tr><th>股票</th><th>方向</th><th>价格</th><th>股数</th><th>时间</th></tr></thead>
          <tbody>
            <tr v-for="t in trades" :key="t.id + t.side">
              <td><RouterLink :to="`/stock/${t.company_id}`">{{ t.title }}</RouterLink></td>
              <td :class="t.side === 'buy' ? 'up' : 'down'">{{ t.side === 'buy' ? '买入' : '卖出' }}</td>
              <td class="num">{{ money(t.price) }}</td>
              <td class="num">{{ t.qty }}</td>
              <td class="num muted">{{ time(t.ts) }}</td>
            </tr>
          </tbody>
        </table>
        <p v-else class="empty">还没有成交</p>
      </div>

      <div v-else class="scroll-x">
        <table class="grid" v-if="ledger.length">
          <thead><tr><th>项目</th><th>相关股票</th><th>金额</th><th>时间</th></tr></thead>
          <tbody>
            <tr v-for="(l, i) in ledger" :key="i">
              <td>{{ LEDGER_REASON[l.reason] ?? l.reason }}</td>
              <td class="muted">{{ l.title ?? '–' }}</td>
              <td class="num" :class="dir(l.amount)">{{ signed(l.amount) }}</td>
              <td class="num muted">{{ time(l.ts) }}</td>
            </tr>
          </tbody>
        </table>
        <p v-else class="empty">还没有资金流水</p>
      </div>
    </section>
  </main>
</template>

<style scoped>
.hero { padding: 14px 12px; display: flex; flex-direction: column; gap: 12px; }
.top { display: flex; flex-direction: column; gap: 2px; }
.total { font-size: 30px; font-weight: 700; line-height: 1.15; }
.stats { margin: 0; display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 8px 12px; }
.stats div { display: flex; flex-direction: column; }
.stats dt { color: var(--muted); }
.stats dd { margin: 0; }
@media (min-width: 720px) { .stats { grid-template-columns: repeat(6, minmax(0, 1fr)); } }
.positions { display: flex; flex-direction: column; }
.pos { display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border-bottom: 1px solid var(--rule); color: var(--ink); }
.pos:hover { background: var(--surface-2); }
.pname { display: flex; flex-direction: column; min-width: 0; }
.pname b { font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pgrid { margin: 0; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 4px 12px; }
.pgrid div { display: flex; justify-content: space-between; gap: 8px; }
.pgrid dt { color: var(--muted); }
.pgrid dd { margin: 0; text-align: right; }
.total-return { font-weight: 600; }
@media (min-width: 720px) { .pgrid { grid-template-columns: repeat(5, minmax(0, 1fr)); } .pgrid div { flex-direction: column; } .pgrid dd { text-align: left; } }
.note { padding: 10px 12px; }
.tabs { padding: 8px; }
.tabs .seg { flex: 1; }
.list { list-style: none; margin: 0; padding: 0; }
.list li { display: grid; grid-template-columns: minmax(0, 1fr) auto auto auto auto; gap: 8px; align-items: center; padding: 10px 12px; border-bottom: 1px solid var(--rule); font-size: 13px; }
.list li:last-child { border-bottom: none; }
.lt { color: var(--ink); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
