<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { errorText } from '../api'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import RestructureEntry from '../components/RestructureEntry.vue'
import Sparkline from '../components/Sparkline.vue'
import Tabs from '../components/Tabs.vue'
import { usePoll } from '../composables/usePoll'
import { dir, money, pct, qty, signed, when } from '../format'
import { useAccount } from '../stores/account'
import { useIpo } from '../stores/ipo'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

const session = useSession()
const account = useAccount()
const prefs = usePrefs()
const ipo = useIpo()
const tab = ref('pos')
const error = ref('')
const cancelling = ref<number | null>(null)
const actionMsg = ref('')

// positions and orders follow the `me` pushes; the poll is the fallback while the socket is down
usePoll(async () => {
  try {
    await Promise.all([session.loadMe(), account.refreshPositions(), account.refreshOrders()])
    error.value = ''
  } catch (e) {
    error.value = errorText(e, '无法加载')
  }
}, 5000, 30000)
usePoll(() => Promise.all([session.loadLeaders('all'), ipo.refresh(), account.refreshHistory()]), 60000)
// the paged lists load when their tab opens, then grow with "加载更多" (trades also by push)
async function openTab(t: string) {
  try {
    if (t === 'trades') await account.refreshTrades()
    if (t === 'ledger') await account.refreshLedger()
  } catch (e) {
    error.value = errorText(e, '无法加载')
  }
}
watch(tab, openTab)
const loadingMore = ref(false)
async function more(which: 'trades' | 'ledger') {
  loadingMore.value = true
  try {
    await (which === 'trades' ? account.moreTrades() : account.moreLedger())
  } catch (e) {
    error.value = errorText(e, '无法加载')
  } finally {
    loadingMore.value = false
  }
}

const me = computed(() => session.me)
function rememberHold() {
  prefs.context = { key: 'hold', label: '持仓', ids: positions.value.map((p) => p.stock.id) }
}
const positions = computed(() => account.positions ?? [])
const today = computed(() => positions.value.reduce((a, p) => a + (p.value - p.value / (1 + (p.day_change || 0))), 0))
const floating = computed(() => positions.value.reduce((a, p) => a + p.pnl, 0))
const rank = computed(() => session.boards.all?.viewer?.rank ?? null)
const pending = computed(() => ipo.open.filter((i) => i.viewer?.quantity).length)
const curve = computed(() => (account.history ?? []).map((d) => d.total_assets))
const tabs = computed(() => [
  { key: 'pos', label: positions.value.length ? `持仓 ${positions.value.length}` : '持仓' },
  { key: 'orders', label: account.orders?.length ? `委托 ${account.orders.length}` : '委托' },
  { key: 'trades', label: '成交' },
  { key: 'ledger', label: '资金明细' },
])

async function cancel(id: number) {
  cancelling.value = id
  actionMsg.value = ''
  try {
    await account.cancel(id)
    actionMsg.value = '已撤单'
  } catch (e) {
    actionMsg.value = errorText(e, '无法撤单')
  } finally {
    cancelling.value = null
  }
}
</script>

<template>
  <main class="page me">
    <header class="pagehead phone-only">
      <h1>我的</h1>
      <span class="who clamp1">{{ me?.username }}</span>
      <RouterLink to="/settings" class="iconbtn gear" aria-label="设置"><Icon name="settings" :size="20" /></RouterLink>
    </header>

    <div class="frame">
      <div class="side">
        <section v-if="me" class="summary gutter">
          <div class="lbl">总资产</div>
          <div class="num total">{{ money(me.total_assets) }}</div>
          <div class="num sub">
            <span><span class="sans muted">今日 </span><span :class="dir(today)">{{ signed(today) }}</span></span>
            <span><span class="sans muted">累计 </span><span :class="dir(me.total_return)">{{ pct(me.total_return) }}</span></span>
            <span v-if="me.restructure.count" class="sans muted">重整 {{ me.restructure.count }} 次</span>
          </div>
          <Sparkline v-if="curve.length > 1" :values="curve" :base="me.initial_cash" :w="320" :h="56" class="curve" role="img"
                     :aria-label="`资产曲线，${curve.length} 个交易日`" />
        </section>
        <dl v-if="me" class="facts split gutter">
          <div><dt>可用资金</dt><dd>{{ money(me.available_cash) }}</dd></div>
          <div><dt>市值</dt><dd>{{ money(me.stock_value) }}</dd></div>
          <div><dt>浮动盈亏</dt><dd :class="dir(floating)">{{ signed(floating) }}</dd></div>
        </dl>
        <RestructureEntry v-if="me" :me="me" />
        <nav class="menu desk-only" aria-label="更多">
          <RouterLink to="/rank"><span>排行</span><span class="num v">{{ rank ? `第 ${rank} 名` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/ipo"><span>新股认购</span><span class="v">{{ pending ? `${pending} 笔待配售` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/guide"><span>新手指南</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/rules"><span>规则</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/settings"><span>设置</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        </nav>
      </div>

      <div class="main">
        <Tabs v-model="tab" :items="tabs" label="账户" class="tabs gutter" />
        <p v-if="error" class="msg error gutter pad">{{ error }}</p>
        <p v-if="actionMsg" class="msg ok gutter pad" role="status">{{ actionMsg }}</p>

        <template v-if="tab === 'pos'">
          <template v-if="account.positions">
            <RouterLink v-for="p in positions" :key="p.stock.id" :to="`/stocks/${p.stock.id}`" class="hold phone-only" @click="rememberHold">
              <Cover :id="p.stock.id" :title="p.stock.title" :w="36" :h="50" :spine="false" class="hc" />
              <span class="serif clamp1 t">{{ p.stock.title }}</span>
              <span class="num r v">{{ money(p.value) }}</span>
              <span class="num m">{{ qty(p.quantity) }} 股 · 成本 {{ money(p.average_cost) }}<template v-if="p.dividends > 0"> · 分红 {{ money(p.dividends) }}</template></span>
              <span class="num r m" :class="dir(p.pnl)">{{ signed(p.pnl) }}&nbsp;&nbsp;{{ pct(p.pnl_ratio) }}</span>
            </RouterLink>
            <table v-if="positions.length" class="table desk-only htable">
              <thead><tr><th>名称</th><th>持仓</th><th>可卖</th><th>成本</th><th>现价</th><th>市值</th><th>盈亏</th><th>今日</th><th>分红</th></tr></thead>
              <tbody>
                <tr v-for="p in positions" :key="p.stock.id">
                  <td class="tt"><RouterLink :to="`/stocks/${p.stock.id}`" class="hn" @click="rememberHold"><Cover :id="p.stock.id" :title="p.stock.title" :w="24" :h="34" :spine="false" /><span class="serif clamp1">{{ p.stock.title }}</span></RouterLink></td>
                  <td class="num">{{ qty(p.quantity) }}</td>
                  <td class="num">{{ qty(p.available_quantity) }}</td>
                  <td class="num">{{ money(p.average_cost) }}</td>
                  <td class="num">{{ money(p.price) }}</td>
                  <td class="num">{{ money(p.value) }}</td>
                  <td class="num" :class="dir(p.pnl)">{{ signed(p.pnl) }}</td>
                  <td class="num" :class="dir(p.day_change)">{{ pct(p.day_change) }}</td>
                  <td class="num">{{ money(p.dividends) }}</td>
                </tr>
              </tbody>
            </table>
            <EmptyState v-if="!positions.length" text="暂无持仓" action="去看股票" to="/stocks" />
          </template>
          <div v-else class="gutter"><div v-for="i in 3" :key="i" class="skel" style="height: 50px; margin: 12px 0"></div></div>
        </template>

        <template v-else-if="tab === 'orders'">
          <table v-if="account.orders?.length" class="table desk-only htable">
            <thead><tr><th>名称</th><th>方向</th><th>价格</th><th>数量</th><th>已成交</th><th>时间</th><th></th></tr></thead>
            <tbody>
              <tr v-for="o in account.orders" :key="o.id">
                <td class="tt"><RouterLink :to="`/stocks/${o.stock.id}`" class="serif clamp1">{{ o.stock.title }}</RouterLink></td>
                <td :class="o.side === 'buy' ? 'up' : 'down'">{{ o.side === 'buy' ? '买入' : '卖出' }}</td>
                <td class="num">{{ o.price != null ? money(o.price) : '市价' }}</td>
                <td class="num">{{ qty(o.quantity) }}</td>
                <td class="num">{{ qty(o.filled_quantity) }}</td>
                <td class="num muted">{{ when(o.created_at, session.status?.trading_day) }}</td>
                <td><button type="button" class="linkbtn" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button></td>
              </tr>
            </tbody>
          </table>
          <div v-for="o in account.orders ?? []" :key="o.id" class="ord phone-only">
            <RouterLink :to="`/stocks/${o.stock.id}`" class="serif clamp1 t">{{ o.stock.title }}</RouterLink>
            <span class="oside" :class="o.side === 'buy' ? 'up' : 'down'">{{ o.side === 'buy' ? '买入' : '卖出' }}</span>
            <span class="num m">{{ o.price != null ? money(o.price) : '市价' }} · 已成交 {{ qty(o.filled_quantity) }}/{{ qty(o.quantity) }}</span>
            <button type="button" class="linkbtn" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button>
          </div>
          <EmptyState v-if="account.orders && !account.orders.length" text="暂无委托" action="去看股票" to="/stocks" />
        </template>

        <template v-else-if="tab === 'trades'">
          <div class="gutter">
            <table v-if="account.trades?.length" class="table">
              <thead><tr><th></th><th>价格</th><th>数量</th><th>时间</th></tr></thead>
              <tbody>
                <tr v-for="t in account.trades" :key="t.id + t.side">
                  <td class="tt"><RouterLink :to="`/stocks/${t.stock.id}`" class="clamp1"><span :class="t.side === 'buy' ? 'up' : 'down'">{{ t.side === 'buy' ? '买' : '卖' }}</span> {{ t.stock.title }}</RouterLink></td>
                  <td class="num">{{ money(t.price) }}</td>
                  <td class="num">{{ qty(t.quantity) }}</td>
                  <td class="num muted">{{ when(t.at, session.status?.trading_day) }}</td>
                </tr>
              </tbody>
            </table>
            <button v-if="account.tradesNext" type="button" class="linkbtn morebtn" :disabled="loadingMore" @click="more('trades')">{{ loadingMore ? '加载中…' : '加载更多' }}</button>
          </div>
          <EmptyState v-if="account.trades && !account.trades.length" text="暂无成交" action="去看股票" to="/stocks" />
        </template>

        <template v-else>
          <div class="gutter">
            <table v-if="account.ledger?.length" class="table">
              <thead><tr><th></th><th>金额</th><th>时间</th></tr></thead>
              <tbody>
                <tr v-for="l in account.ledger" :key="l.id">
                  <td class="tt"><span class="clamp1">{{ l.reason_label }}<span v-if="l.stock" class="muted"> · {{ l.stock.title }}</span></span></td>
                  <td class="num" :class="dir(l.amount)">{{ signed(l.amount) }}</td>
                  <td class="num muted">{{ when(l.at, session.status?.trading_day) }}</td>
                </tr>
              </tbody>
            </table>
            <button v-if="account.ledgerNext" type="button" class="linkbtn morebtn" :disabled="loadingMore" @click="more('ledger')">{{ loadingMore ? '加载中…' : '加载更多' }}</button>
          </div>
          <EmptyState v-if="account.ledger && !account.ledger.length" text="暂无资金明细" />
        </template>

        <nav class="menu phone-only" aria-label="更多">
          <RouterLink to="/rank"><span>排行</span><span class="num v">{{ rank ? `第 ${rank} 名` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/ipo"><span>新股认购</span><span class="v">{{ pending ? `${pending} 笔待配售` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/guide"><span>新手指南</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/rules"><span>规则</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        </nav>
      </div>
    </div>
  </main>
</template>

<style scoped>
.curve { margin-top: 12px; width: 100%; max-width: 320px; }
.morebtn { display: block; margin: 12px auto 4px; font-size: 13px; }
.who { font-size: 13px; color: var(--ink-2); min-width: 0; }
.gear { margin-left: auto; width: 36px; height: 36px; }
.summary { padding-top: 18px; padding-bottom: 14px; display: flex; flex-direction: column; gap: 6px; }
.lbl { font-size: 12px; color: var(--muted); }
.total { font-size: 36px; font-weight: 600; letter-spacing: -0.02em; line-height: 1.05; }
.sub { display: flex; gap: 16px; font-size: 13px; }
.sans { font-family: var(--font); }
.split { border-top: 1px solid var(--hair); border-bottom: 1px solid var(--rule); }
.tabs { padding-top: 12px; }
.pad { padding-top: 10px; }
.hold { display: grid; grid-template-columns: 36px minmax(0, 1fr) auto; column-gap: 12px; row-gap: 4px; padding: 12px var(--gutter); border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.hc { grid-row: span 2; }
.t { font-size: 15px; font-weight: 600; color: var(--ink); min-width: 0; }
.v { font-size: 15px; }
.r { text-align: right; }
.m { font-size: 12px; color: var(--muted); }
.m.up { color: var(--up); }
.m.down { color: var(--down); }
.ord { display: grid; grid-template-columns: minmax(0, 1fr) auto; row-gap: 4px; column-gap: 12px; padding: 12px var(--gutter); border-bottom: 1px solid var(--hair-2); align-items: center; }
.oside { font-size: 13px; text-align: right; }
.tt { max-width: 0; width: 50%; }
.tt a, .tt > span { display: block; }
.menu { margin-top: 18px; border-top: 1px solid var(--rule); display: flex; flex-direction: column; }
.menu a { display: flex; align-items: center; padding: 14px var(--gutter); border-bottom: 1px solid var(--hair-2); font-size: 15px; color: var(--ink); }
.menu .v { margin-left: auto; font-size: 13px; color: var(--ink-2); }
.chev { margin-left: 6px; color: var(--hold-npc); }

@media (min-width: 1024px) {
  .frame { max-width: calc(var(--container) + 64px); margin: 0 auto; padding: 0 32px; display: grid; grid-template-columns: 340px minmax(0, 1fr); column-gap: 40px; align-items: start; }
  .side { --gutter: 0px; position: sticky; top: var(--desk-header-h); padding-bottom: 24px; }
  .main { --gutter: 0px; min-width: 0; padding-top: 18px; padding-bottom: 48px; }
  .summary { padding-top: 28px; }
  .total { font-size: 38px; }
  .htable td { padding-top: 12px; padding-bottom: 12px; font-size: 14px; }
  .tt .hn { display: flex; align-items: center; gap: 10px; min-width: 0; color: var(--ink); }
  .hn .serif, .tt .serif { font-weight: 600; min-width: 0; }
  .tt { width: 30%; }
  .menu.desk-only { display: flex !important; }
}
</style>
