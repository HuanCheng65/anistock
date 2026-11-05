<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { ApiError } from '../api'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import Tabs from '../components/Tabs.vue'
import { usePoll } from '../composables/usePoll'
import { dir, LEDGER_REASON, money, pct, qty, signed, when } from '../format'
import { useAccount } from '../stores/account'
import { useIpo } from '../stores/ipo'
import { useSession } from '../stores/session'

const session = useSession()
const account = useAccount()
const ipo = useIpo()
const tab = ref('pos')
const error = ref('')
const cancelling = ref<number | null>(null)
const actionMsg = ref('')

const { refresh } = usePoll(async () => {
  try {
    const jobs: Promise<unknown>[] = [session.loadMe(), account.refreshPositions(), account.refreshOrders()]
    if (tab.value === 'trades') jobs.push(account.refreshTrades())
    if (tab.value === 'ledger') jobs.push(account.refreshLedger())
    await Promise.all(jobs)
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '无法加载'
  }
}, 5000)
usePoll(() => Promise.all([session.loadLeaders(), ipo.refresh()]), 60000)
watch(tab, () => refresh())

const me = computed(() => session.me)
const positions = computed(() => account.positions ?? [])
const today = computed(() => positions.value.reduce((a, p) => a + (p.value - p.value / (1 + (p.day_change || 0))), 0))
const floating = computed(() => positions.value.reduce((a, p) => a + p.pnl, 0))
const rank = computed(() => session.leaders?.find((r) => r.username === me.value?.username)?.rank ?? null)
const pending = computed(() => (ipo.ipos ?? []).filter((i) => i.status === 'open' && i.my?.qty).length)
const tabs = computed(() => [
  { key: 'pos', label: '持仓' },
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
    actionMsg.value = e instanceof ApiError ? e.message : '无法撤单'
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
          <div class="num total">{{ money(me.total) }}</div>
          <div class="num sub">
            <span><span class="sans muted">今日 </span><span :class="dir(today)">{{ signed(today) }}</span></span>
            <span><span class="sans muted">累计 </span><span :class="dir(me.return_pct)">{{ pct(me.return_pct) }}</span></span>
          </div>
        </section>
        <dl v-if="me" class="facts split gutter">
          <div><dt>可用资金</dt><dd>{{ money(me.available) }}</dd></div>
          <div><dt>市值</dt><dd>{{ money(me.stock_value) }}</dd></div>
          <div><dt>浮动盈亏</dt><dd :class="dir(floating)">{{ signed(floating) }}</dd></div>
        </dl>
        <nav class="menu desk-only" aria-label="更多">
          <RouterLink to="/rank"><span>排行</span><span class="num v">{{ rank ? `第 ${rank} 名` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/ipo"><span>新股认购</span><span class="v">{{ pending ? `${pending} 笔待配售` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/guide"><span>新手指南</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/rules"><span>规则</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/macro"><span>宏观</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/settings"><span>设置</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        </nav>
      </div>

      <div class="main">
        <Tabs v-model="tab" :items="tabs" label="账户" class="tabs gutter" />
        <p v-if="error" class="msg error gutter pad">{{ error }}</p>
        <p v-if="actionMsg" class="msg ok gutter pad" role="status">{{ actionMsg }}</p>

        <template v-if="tab === 'pos'">
          <template v-if="account.positions">
            <RouterLink v-for="p in positions" :key="p.company_id" :to="`/stock/${p.company_id}`" class="hold">
              <Cover :id="p.company_id" :title="p.title" :w="36" :h="50" :spine="false" class="hc" />
              <span class="serif clamp1 t">{{ p.title }}</span>
              <span class="num r v">{{ money(p.value) }}</span>
              <span class="num m">{{ qty(p.qty) }} 股 · 成本 {{ money(p.avg_cost) }}<template v-if="p.dividends > 0"> · 分红 {{ money(p.dividends) }}</template></span>
              <span class="num r m" :class="dir(p.pnl)">{{ signed(p.pnl) }}&nbsp;&nbsp;{{ pct(p.pnl_pct) }}</span>
            </RouterLink>
            <EmptyState v-if="!positions.length" text="暂无持仓" action="去看行情" to="/" />
          </template>
          <div v-else class="gutter"><div v-for="i in 3" :key="i" class="skel" style="height: 50px; margin: 12px 0"></div></div>
        </template>

        <template v-else-if="tab === 'orders'">
          <div v-for="o in account.orders ?? []" :key="o.id" class="ord">
            <RouterLink :to="`/stock/${o.company_id}`" class="serif clamp1 t">{{ o.title }}</RouterLink>
            <span class="oside" :class="o.side === 'buy' ? 'up' : 'down'">{{ o.side === 'buy' ? '买入' : '卖出' }}</span>
            <span class="num m">{{ o.price != null ? money(o.price) : '市价' }} · 已成交 {{ qty(o.filled) }}/{{ qty(o.qty) }}</span>
            <button type="button" class="linkbtn" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button>
          </div>
          <EmptyState v-if="account.orders && !account.orders.length" text="暂无委托" action="去看行情" to="/" />
        </template>

        <template v-else-if="tab === 'trades'">
          <div class="gutter">
            <table v-if="account.trades?.length" class="table">
              <thead><tr><th></th><th>价格</th><th>数量</th><th>时间</th></tr></thead>
              <tbody>
                <tr v-for="t in account.trades" :key="t.id + t.side">
                  <td class="tt"><RouterLink :to="`/stock/${t.company_id}`" class="clamp1"><span :class="t.side === 'buy' ? 'up' : 'down'">{{ t.side === 'buy' ? '买' : '卖' }}</span> {{ t.title }}</RouterLink></td>
                  <td class="num">{{ money(t.price) }}</td>
                  <td class="num">{{ qty(t.qty) }}</td>
                  <td class="num muted">{{ when(t.ts, session.status?.day) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <EmptyState v-if="account.trades && !account.trades.length" text="暂无成交" action="去看行情" to="/" />
        </template>

        <template v-else>
          <div class="gutter">
            <table v-if="account.ledger?.length" class="table">
              <thead><tr><th></th><th>金额</th><th>时间</th></tr></thead>
              <tbody>
                <tr v-for="(l, i) in account.ledger" :key="i">
                  <td class="tt"><span class="clamp1">{{ LEDGER_REASON[l.reason] ?? l.reason }}<span v-if="l.title" class="muted"> · {{ l.title }}</span></span></td>
                  <td class="num" :class="dir(l.amount)">{{ signed(l.amount) }}</td>
                  <td class="num muted">{{ when(l.ts, session.status?.day) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <EmptyState v-if="account.ledger && !account.ledger.length" text="暂无资金明细" />
        </template>

        <nav class="menu phone-only" aria-label="更多">
          <RouterLink to="/rank"><span>排行</span><span class="num v">{{ rank ? `第 ${rank} 名` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/ipo"><span>新股认购</span><span class="v">{{ pending ? `${pending} 笔待配售` : '' }}</span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/guide"><span>新手指南</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/rules"><span>规则</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
          <RouterLink to="/macro"><span>宏观</span><span class="v"></span><Icon name="chevron" :size="16" class="chev" /></RouterLink>
        </nav>
      </div>
    </div>
  </main>
</template>

<style scoped>
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
  .frame { max-width: 1200px; margin: 0 auto; display: grid; grid-template-columns: 360px minmax(0, 1fr); align-items: start; }
  .side { border-right: 1px solid var(--hair); min-height: calc(100vh - var(--desk-header-h)); --gutter: 24px; }
  .main { --gutter: 28px; min-width: 0; }
  .summary { padding-top: 24px; }
  .menu.desk-only { display: flex !important; }
}
</style>
