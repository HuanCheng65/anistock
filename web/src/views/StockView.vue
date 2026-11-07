<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import BottomNav from '../components/BottomNav.vue'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import IpoOffer from '../components/IpoOffer.vue'
import RightsOffer from '../components/RightsOffer.vue'
import KChart, { type Period } from '../components/KChart.vue'
import OrderBook from '../components/OrderBook.vue'
import OrderForm from '../components/OrderForm.vue'
import BizPanel from '../components/stock/BizPanel.vue'
import HeatPanel from '../components/stock/HeatPanel.vue'
import HoldersPanel from '../components/stock/HoldersPanel.vue'
import NoticePanel from '../components/stock/NoticePanel.vue'
import PositionStrip from '../components/stock/PositionStrip.vue'
import WorksPanel from '../components/stock/WorksPanel.vue'
import Tabs from '../components/Tabs.vue'
import TradeTape from '../components/TradeTape.vue'
import ListSidebar from '../components/ListSidebar.vue'
import { DESK, useMedia } from '../composables/useMedia'
import { usePoll } from '../composables/usePoll'
import { useStockNav } from '../composables/useStockNav'
import { bigMoney, dir, hm, money, pct, qty, share, signed, weekdayTime } from '../format'
import { useAccount } from '../stores/account'
import { useCredit } from '../stores/credit'
import { useFutures } from '../stores/futures'
import { markEditing } from '../version'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'
import { useStock } from '../stores/stock'
import { liveState, useTopic } from '../live'

const route = useRoute()
const router = useRouter()
const store = useStock()
const market = useMarket()
const prefs = usePrefs()
const session = useSession()
const account = useAccount()
const credit = useCredit()
const fut = useFutures()
usePoll(() => fut.refreshList().catch(() => {}), 300000)
/** the heat futures contract on this stock's work, while one trades */
const contract = computed(() => fut.forStock(id.value))
const desk = useMedia(DESK)
const wide = useMedia('(min-width: 1180px)')
const drawer = ref(false)
const id = computed(() => Number(route.params.id))

const period = ref<Period>('day')
const pick = ref<{ price: number; n: number } | null>(null)
const sheet = ref<'buy' | 'sell' | null>(null)

const { refresh } = usePoll(async () => {
  await store.refresh(id.value)
  if (period.value === 'intraday') await store.refreshIntraday(id.value)
}, 5000, 20000)
// the player's position and open orders here (the order form and the 持仓 block)
usePoll(() => (session.me ? Promise.all([
  account.refreshPositions(), account.refreshOrders(),
  // financing owed and shares owed here (持仓), only with a credit account
  session.me.credit_status != null ? credit.refresh().catch(() => {}) : null,
]) : null), 15000, 60000)
watch(sheet, (v) => markEditing('order-sheet', !!v))
onBeforeUnmount(() => markEditing('order-sheet', false))
useTopic(computed(() => (Number.isFinite(id.value) ? `stock:${id.value}` : null)))
usePoll(() => (market.stocks ? null : market.refreshQuotes()), 60000)
watch(period, (p) => p === 'intraday' && store.refreshIntraday(id.value).catch(() => {}))

const s = computed(() => store.detail[id.value] ?? null)
const book = computed(() => store.book[id.value] ?? { bids: [], asks: [], auction: null })
const tape = computed(() => store.trades[id.value] ?? [])
const q = computed(() => s.value?.quote ?? null)
const err = computed(() => store.error[id.value] ?? '')
const change = computed(() => q.value?.change ?? 0)
const aired = computed(() => s.value?.episodes_aired ?? 0)
const segs = computed(() => {
  const n = s.value?.episodes
  if (!n || s.value?.is_long_runner || n > 52) return null
  return Array.from({ length: n }, (_, i) => i < aired.value)
})
const progress = computed(() => {
  const st = s.value
  if (!st) return ''
  if (!aired.value) return '未开播'
  if (st.is_long_runner || !st.episodes) return `第 ${aired.value} 集`
  return `${Math.min(aired.value, st.episodes)} / ${st.episodes}`
})
const slot = computed(() => (s.value?.next_episode ? weekdayTime(s.value.next_episode.airs_at) : ''))
const stats = computed(() => {
  const x = q.value
  if (!x) return []
  const c = (v: number | null) => (v == null ? '' : dir(v - x.prev_close))
  const base = [
    { k: '今开', v: money(x.open), c: c(x.open), n: x.open },
    { k: '最高', v: money(x.high), c: c(x.high), n: x.high },
    { k: '最低', v: money(x.low), c: c(x.low), n: x.low },
  ]
  const tail = [
    { k: '成交量', v: qty(x.volume), c: '', n: x.volume },
    { k: '成交额', v: bigMoney(x.turnover), c: '', n: x.turnover },
    { k: '市值', v: bigMoney(x.market_cap), c: '', n: null },
  ]
  return desk.value
    ? [...base, { k: '昨收', v: money(x.prev_close), c: '', n: null }, ...tail,
       { k: '市盈率(TTM)', v: s.value?.ratios.pe_ttm != null ? s.value.ratios.pe_ttm.toFixed(1) : '–', c: '', n: null },
       { k: '股息率(TTM)', v: s.value?.ratios.dividend_yield_ttm != null ? share(s.value.ratios.dividend_yield_ttm, 1) : '–', c: '', n: null }]
    : [...base, ...tail]
})
const periods = [
  { key: 'intraday', label: '分时' },
  { key: 'day', label: '日 K' },
  { key: 'week', label: '周 K' },
  { key: 'month', label: '月 K' },
]
const periodModel = computed({ get: () => period.value as string, set: (v: string) => (period.value = v as Period) })

const pos = computed(() => (session.me ? (account.positions ?? []).find((x) => x.stock.id === id.value) ?? null : null))
const subscribing = computed(() => s.value?.status === 'subscribing')

// ---- the lower half: 热度 / 经营 / 作品 / 公告 / 股东, the last one chosen remembered for the session
type Lower = 'heat' | 'biz' | 'works' | 'notices' | 'holders'
const lowerTabs: { key: Lower; label: string }[] = [
  { key: 'heat', label: '热度' },
  { key: 'biz', label: '经营' },
  { key: 'works', label: '作品' },
  { key: 'notices', label: '公告' },
  { key: 'holders', label: '股东' },
]
const TAB_KEY = 'stock.tab'
function savedTab(): Lower | null {
  try {
    const v = sessionStorage.getItem(TAB_KEY)
    return lowerTabs.some((t) => t.key === v) ? (v as Lower) : null
  } catch {
    return null
  }
}
const chosen = ref<Lower | null>(savedTab())
// before a choice: a stock still in subscription has no revenue yet, so it opens on its schedule
const tab = computed<Lower>(() => chosen.value ?? (subscribing.value ? 'works' : 'heat'))
const tabModel = computed({ get: () => tab.value as string, set: (v: string) => (chosen.value = v as Lower) })
const lower = ref<HTMLElement | null>(null)
const tabbar = ref<HTMLElement | null>(null)
// switching while the tab bar is stuck keeps it where it is: the new panel starts right under it
watch(tab, async (t) => {
  try {
    sessionStorage.setItem(TAB_KEY, t)
  } catch { /* private mode */ }
  const bar = tabbar.value
  const sec = lower.value
  if (!bar || !sec) return
  const stick = parseFloat(getComputedStyle(bar).top) || 0
  const stuck = bar.getBoundingClientRect().top <= stick + 1 && sec.getBoundingClientRect().top < stick
  await nextTick()
  if (stuck) window.scrollTo({ top: window.scrollY + sec.getBoundingClientRect().top - stick })
}, { flush: 'pre' })
const heatFailed = ref(false)
function loadHeat(force = false) {
  heatFailed.value = false
  store.refreshHeat(id.value, force).catch(() => (heatFailed.value = true))
}
const worksFailed = ref(false)
function loadWorks(force = false) {
  worksFailed.value = false
  store.refreshWorks(id.value, force).catch(() => (worksFailed.value = true))
}
usePoll(() => {
  if (!Number.isFinite(id.value)) return null
  if (tab.value === 'works') loadWorks()
  if (tab.value === 'heat') loadHeat()
  if (tab.value === 'holders') return credit.loadMargin(id.value).catch(() => {})
  return null
}, 60000)
watch([tab, id], () => {
  if (tab.value === 'works' && !store.works[id.value]) loadWorks()
  if (tab.value === 'heat' && !store.heat[id.value]) loadHeat()
  if (tab.value === 'holders' && !credit.margins[id.value]) credit.loadMargin(id.value).catch(() => {})
})
const works = computed(() => store.works[id.value] ?? null)
const margin = computed(() => credit.margins[id.value] ?? null)
const notices = computed(() => store.news[id.value] ?? null)
const creditAcct = computed(() => (session.me?.credit_status != null ? credit.account : null))
const rights = computed(() => store.rights[id.value] ?? null)
const exDays = computed(() => store.exDays[id.value] ?? [])
async function setAdjust(v: 'forward' | 'none') {
  if (store.adjust === v) return
  store.adjust = v
  await store.refresh(id.value, true).catch(() => {})
}
// the cooling-off ends at `until`: count down every second, and drop the banner when it ends
const nowMs = ref(Date.now())
const clockTimer = window.setInterval(() => (nowMs.value = Date.now()), 1000)
onBeforeUnmount(() => window.clearInterval(clockTimer))
const vcmEnd = computed(() => (s.value?.vcm.active && s.value.vcm.until ? Date.parse(s.value.vcm.until) : null))
const vcm = computed(() => (s.value?.vcm.active && (vcmEnd.value == null || vcmEnd.value > nowMs.value) ? s.value.vcm : null))
const vcmLeft = computed(() => {
  if (vcmEnd.value == null) return ''
  const sec = Math.max(0, Math.round((vcmEnd.value - nowMs.value) / 1000))
  return `${Math.floor(sec / 60)}:${String(sec % 60).padStart(2, '0')}`
})
const liveChart = computed(() => session.status?.phase === 'continuous' && liveState.value === 'open')
const auction = computed(() => (session.status?.phase === 'auction' ? book.value.auction : null))
const watching = computed(() => prefs.watching(id.value))

function onPick(price: number) {
  pick.value = { price, n: (pick.value?.n ?? 0) + 1 }
  if (!desk.value && !sheet.value) sheet.value = 'buy'
}
async function afterOrder() {
  await refresh()
  session.loadMe().catch(() => {})
}
const nav = useStockNav(id)
function go(sid: number | undefined) {
  if (sid) router.replace(`/stocks/${sid}`)
}
function onKey(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null
  if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
  if (e.altKey || e.ctrlKey || e.metaKey) return
  if (e.key === 'ArrowUp' && nav.prev.value) { e.preventDefault(); go(nav.prev.value.stock.id) }
  if (e.key === 'ArrowDown' && nav.next.value) { e.preventDefault(); go(nav.next.value.stock.id) }
}
onMounted(() => window.addEventListener('keydown', onKey))
onBeforeUnmount(() => window.removeEventListener('keydown', onKey))
watch(id, () => (drawer.value = false))
function back() {
  if (window.history.state?.back) router.back()
  else router.push('/')
}
</script>

<template>
  <main class="page stockpage" :class="{ sub: subscribing }">
    <header class="pagehead back phone-only">
      <button type="button" class="iconbtn" aria-label="返回" @click="back"><Icon name="back" /></button>
      <span class="num code">{{ s?.stock.code }}</span>
      <button type="button" class="iconbtn step" aria-label="上一支" :disabled="!nav.prev.value" @click="go(nav.prev.value?.stock.id)"><Icon name="back" :size="20" /></button>
      <button type="button" class="iconbtn step" aria-label="下一支" :disabled="!nav.next.value" @click="go(nav.next.value?.stock.id)"><Icon name="chevron" :size="20" /></button>
      <button v-if="s" type="button" class="iconbtn star" :class="{ on: watching }" :aria-label="watching ? '移出自选' : '加入自选'" :aria-pressed="watching" @click="prefs.toggle(id)">
        <Icon name="watch" :size="21" :fill="watching" />
      </button>
    </header>

    <div class="frame" :class="{ collapsed: prefs.sideCollapsed && wide }">
      <ListSidebar v-if="wide && !prefs.sideCollapsed" v-model:tab="nav.tab.value" class="left" :rows="nav.rows.value" :tabs="nav.tabs.value" :current="id" collapsible @collapse="prefs.sideCollapsed = true" />
      <div v-else-if="wide" class="left rail0"><button type="button" class="cbtn" aria-label="展开列表" @click="prefs.sideCollapsed = false"><Icon name="chevron" :size="16" /></button></div>
      <div v-if="drawer && desk && !wide" class="scrim dscrim" @click.self="drawer = false">
        <ListSidebar v-model:tab="nav.tab.value" class="drawer" :rows="nav.rows.value" :tabs="nav.tabs.value" :current="id" @pick="drawer = false" />
      </div>

      <div class="center">
        <EmptyState v-if="err && !s" :text="err" action="返回股票" to="/stocks" tone="error" />
        <template v-else-if="s && q">
          <nav v-if="contract" class="kind" aria-label="品种">
            <span aria-current="page">股票</span>
            <RouterLink :to="`/futures/${contract.id}`">热度期货</RouterLink>
          </nav>
          <section class="hero">
            <Cover :id="s.stock.id" :title="s.stock.title" :w="desk ? 92 : 96" :h="desk ? 128 : 134" :size="desk ? 18 : 19" />
            <div class="ht">
              <div class="deskmeta desk-only">
                <button v-if="!wide" type="button" class="star2" @click="drawer = true">列表</button>
                <span class="num">{{ s.stock.code }}</span><span v-if="s.studio">{{ s.studio }}</span><span v-if="slot">{{ slot }}</span><span class="num">{{ progress }}</span>
                <button type="button" class="star2" :class="{ on: watching }" :aria-pressed="watching" @click="prefs.toggle(id)">{{ watching ? '已自选' : '加入自选' }}</button>
              </div>
              <h1 class="serif">{{ s.stock.title }}</h1>
              <div v-if="s.title_ja && s.title_ja !== s.stock.title" class="ja clamp2">{{ s.title_ja }}</div>
              <div class="studio phone-only" v-if="s.studio || slot">{{ [s.studio, slot].filter(Boolean).join(' · ') }}</div>
              <div class="eps phone-only">
                <div v-if="segs" class="segs" :style="{ gridTemplateColumns: `repeat(${segs.length}, minmax(0, 1fr))` }">
                  <span v-for="(on, i) in segs" :key="i" :class="{ on }"></span>
                </div>
                <div class="num prog">{{ progress }}</div>
              </div>
            </div>
            <div class="price desk-only">
              <span v-flash="q.price" class="num big" :class="dir(change)">{{ money(q.price) }}</span>
              <span v-flash="q.price" class="num" :class="dir(change)">{{ signed(q.price - q.prev_close) }}&nbsp;&nbsp;{{ pct(change) }}</span>
            </div>
          </section>

          <section class="quote">
            <div class="pline phone-only">
              <span v-flash="q.price" class="num big" :class="dir(change)">{{ money(q.price) }}</span>
              <span v-flash="q.price" class="num" :class="dir(change)">{{ signed(q.price - q.prev_close) }}&nbsp;&nbsp;{{ pct(change) }}</span>
            </div>
            <dl class="stats">
              <div v-for="x in stats" :key="x.k"><dt>{{ x.k }}</dt><dd v-flash.neutral="x.n" class="num" :class="x.c">{{ x.v }}</dd></div>
            </dl>
            <p v-if="vcm" class="banner">冷静期至 <span class="num">{{ hm(vcm.until) }}</span><template v-if="vcmLeft">，剩余 <span class="num">{{ vcmLeft }}</span></template>，只在 <span class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</span> 之间成交</p>
            <p v-if="auction" class="banner">集合竞价中<template v-if="auction.price != null">，参考开盘价 <span class="num">{{ money(auction.price) }}</span>，可成交 <span class="num">{{ qty(auction.quantity) }}</span> 股</template></p>
            <p v-if="s.status === 'delisted'" class="banner">已退市</p>
          </section>

          <section v-if="(store.candles[id] ?? []).length || !subscribing" class="chartsec">
            <Tabs v-model="periodModel" :items="periods" label="周期" :size="14" class="ptabs">
              <span class="legend desk-only">
                <span><i class="dash"></i>播出</span>
                <span><i class="dia"></i>分红</span>
              </span>
              <span v-if="exDays.length && period !== 'intraday'" class="adj" role="group" aria-label="复权">
                <button type="button" :class="{ on: store.adjust === 'forward' }" :aria-pressed="store.adjust === 'forward'" @click="setAdjust('forward')">前复权</button>
                <button type="button" :class="{ on: store.adjust === 'none' }" :aria-pressed="store.adjust === 'none'" @click="setAdjust('none')">不复权</button>
              </span>
            </Tabs>
            <KChart :candles="store.candles[id] ?? []" :period="period" :intraday="store.intraday[id]" :prev-close="q.prev_close"
                    :episodes="s.episode_days" :dividends="s.dividends" :wide="desk" :live="liveChart" />
          </section>

          <!-- phone: book and tape on the terminal surface -->
          <section v-if="!subscribing" class="term booksec phone-only">
            <OrderBook :bids="book.bids" :asks="book.asks" layout="split" @pick="onPick" />
            <TradeTape :trades="tape" :limit="5" class="tape" />
          </section>

          <section v-if="rights" class="sec gutter">
            <h2 class="h2">配股认购</h2>
            <RightsOffer :stock-id="id" :offer="rights" @changed="refresh" />
          </section>

          <section v-if="subscribing && s.ipo" class="sec gutter ipo phone-only">
            <h2 class="h2">新股认购</h2>
            <IpoOffer :ipo="s.ipo" :episodes="s.episodes" compact @changed="refresh" />
          </section>

          <PositionStrip :pos="pos" :credit="creditAcct" :stock-id="id" />

          <section ref="lower" class="lower" aria-label="资料">
            <div ref="tabbar" class="tabbar">
              <Tabs v-model="tabModel" :items="lowerTabs" label="资料" ids="stk" class="ltabs" />
            </div>
            <div :id="`stk-panel-${tab}`" :key="tab" class="panel gutter" role="tabpanel" :aria-labelledby="`stk-tab-${tab}`" tabindex="0">
              <HeatPanel v-if="tab === 'heat'" :s="s" :days="store.heat[id] ?? null" :failed="heatFailed" :today="session.status?.trading_day" :desk="desk" @retry="loadHeat(true)" />
              <BizPanel v-else-if="tab === 'biz'" :s="s" :today="session.status?.trading_day" :desk="desk" />
              <WorksPanel v-else-if="tab === 'works'" :s="s" :works="works" :failed="worksFailed" :today="session.status?.trading_day" @retry="loadWorks(true)" />
              <NoticePanel v-else-if="tab === 'notices'" :items="notices" :more="!!store.newsMore[id]" :today="session.status?.trading_day" :load="() => store.moreNews(id)" :title="s.stock.title" />
              <HoldersPanel v-else :s="s" :margin="margin" />
            </div>
          </section>
        </template>
        <div v-else class="gutter" style="padding-top: 18px">
          <div class="skel" style="height: 134px"></div>
          <div class="skel" style="height: 230px; margin-top: 16px"></div>
        </div>
      </div>

      <aside v-if="s && q" class="desk-only right term" aria-label="交易">
        <template v-if="!subscribing">
          <section class="tsec"><OrderBook :bids="book.bids" :asks="book.asks" layout="stack" :last="q.price" :prev-close="q.prev_close" @pick="onPick" /></section>
          <section class="tsec"><OrderForm :stock="s" :book="book" :pick="pick" @placed="afterOrder" /></section>
          <section class="tsec last"><TradeTape :trades="tape" :limit="12" /></section>
        </template>
        <section v-else-if="s.ipo" class="tsec ipodesk">
          <IpoOffer :ipo="s.ipo" :episodes="s.episodes" compact @changed="refresh" />
        </section>
      </aside>
    </div>

    <BottomNav v-if="subscribing" />
    <!-- phone trade bar and order sheet -->
    <template v-if="s && q && !subscribing && !desk">
      <div class="tradebar term">
        <div class="cash">
          <span>可用资金</span>
          <span class="num">{{ session.me ? money(session.me.available_cash) : '–' }}</span>
        </div>
        <button type="button" class="tbtn sell" @click="sheet = 'sell'">卖出</button>
        <button type="button" class="tbtn buy" @click="sheet = 'buy'">买入</button>
      </div>
      <div v-if="sheet" class="scrim" @click.self="sheet = null">
        <div class="sheet term" role="dialog" aria-label="下单">
          <div class="sheethead">
            <span class="clamp1 serif">{{ s.stock.title }}</span>
            <span class="num" :class="change > 0 ? 'tup' : change < 0 ? 'tdown' : ''">{{ money(q.price) }}</span>
            <button type="button" class="iconbtn x" aria-label="关闭" @click="sheet = null"><Icon name="close" :size="20" /></button>
          </div>
          <OrderForm :stock="s" :book="book" :pick="pick" :initial-side="sheet" @placed="afterOrder" />
        </div>
      </div>
    </template>
  </main>
</template>

<style scoped>
.code { font-size: 13px; color: var(--ink-2); }
.kind { display: flex; gap: 22px; padding: 0 var(--gutter); border-bottom: 1px solid var(--hair); font-size: 15px; }
.kind a, .kind span { padding: 11px 0 9px; color: var(--muted); border-bottom: 2px solid transparent; }
.kind [aria-current] { color: var(--ink); font-weight: 700; border-bottom-color: var(--ink); }
.step { margin-left: auto; width: 36px; }
.step + .step { margin-left: 0; }
.step:disabled { color: var(--hair); }
.star { margin-left: 0; }
.star.on { color: var(--ink); }

.hero { display: flex; gap: 14px; padding: 18px var(--gutter) 16px; }
.ht { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 6px; }
.ht h1 { font-size: 22px; font-weight: 900; line-height: 1.3; overflow-wrap: anywhere; display: -webkit-box; -webkit-line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; }
.ja { font-size: 12px; color: var(--muted); overflow-wrap: anywhere; }
.studio { font-size: 13px; color: var(--ink-2); }
.eps { margin-top: auto; display: flex; flex-direction: column; gap: 4px; }
.segs { display: grid; gap: 2px; }
.segs span { height: 5px; background: var(--hair); }
.segs span.on { background: var(--accent); }
.prog { font-size: 12px; color: var(--ink-2); }

.quote { padding: 0 var(--gutter) 14px; display: flex; flex-direction: column; gap: 10px; }
.pline { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; font-size: 15px; }
.big { font-size: 40px; font-weight: 600; letter-spacing: -0.02em; line-height: 1.1; }
.stats { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); row-gap: 8px; column-gap: 12px; font-size: 12px; }
.stats div { display: flex; justify-content: space-between; gap: 6px; min-width: 0; }
.stats dt { color: var(--muted); white-space: nowrap; }
.stats dd.flat { color: var(--ink); }
.banner { font-size: 13px; color: var(--ink); border-left: 3px solid var(--accent); padding: 4px 0 4px 10px; }

.chartsec { border-top: 1px solid var(--rule); padding-top: 10px; }
.ptabs { padding: 0 var(--gutter); gap: 18px; border-bottom: 0; margin-bottom: 4px; }
.legend { margin-left: auto; gap: 14px; font-size: 13px; color: var(--ink-2); padding-bottom: 10px; }
.legend.desk-only { display: none; }
.legend span { display: inline-flex; align-items: center; gap: 6px; margin-left: 14px; }
.dash { width: 12px; border-top: 1px dashed var(--accent); display: inline-block; }
.dia { width: 7px; height: 7px; background: var(--ink); transform: rotate(45deg); display: inline-block; }
.adj { display: inline-flex; margin-left: 14px; padding-bottom: 10px; }
.adj button { font-size: 12px; color: var(--ink-2); background: none; border: 1px solid var(--hair); padding: 2px 8px; cursor: pointer; }
.adj button + button { border-left: 0; }
.adj button.on { color: var(--ink); border-color: var(--ink); }
.adj button.on + button { border-left: 1px solid var(--hair); }

.booksec { margin: 4px var(--gutter) 0; padding: 12px; }
.tape { margin-top: 10px; padding-top: 8px; border-top: 1px solid var(--t-line); }

.sec { border-top: 1px solid var(--rule); padding-top: 14px; padding-bottom: 14px; display: flex; flex-direction: column; gap: 12px; margin-top: 18px; }
.pos + .lower, .sec + .lower { margin-top: 0; }

/* the lower half: tabs stay at the top of the screen while their panel scrolls */
.lower { margin-top: 18px; --stick: 0px; }
.tabbar { position: sticky; top: var(--stick); z-index: 6; background: var(--ground); border-top: 1px solid var(--rule); padding: 0 var(--gutter); }
.ltabs { gap: 26px; border-bottom-color: var(--hair); }
.ltabs :deep(button) { font-size: 16px; padding: 12px 0 11px; }
.panel { padding-top: 16px; padding-bottom: 24px; min-height: calc(100vh - 120px); animation: panel-in var(--dur-2) var(--ease-out); }
.panel:focus-visible { outline: none; }
@keyframes panel-in { from { opacity: 0; transform: translateY(6px); } }
@media (prefers-reduced-motion: reduce) { .panel { animation: none; } }

.tradebar {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 30; display: flex; align-items: center; gap: 10px;
  padding: 10px var(--gutter) calc(10px + env(safe-area-inset-bottom, 0px));
}
.cash { flex: 1; display: flex; flex-direction: column; gap: 1px; font-size: 11px; color: var(--t-ink-2); min-width: 0; }
.cash .num { font-size: 14px; color: var(--t-ink); }
.tradebar .tbtn { width: 96px; height: 44px; }
.scrim { position: fixed; inset: 0; z-index: 60; background: rgba(0, 0, 0, 0.45); display: flex; align-items: flex-end; }
.sheet { width: 100%; max-height: 92vh; overflow: auto; padding: 4px var(--gutter) calc(16px + env(safe-area-inset-bottom, 0px)); }
.sheethead { display: flex; align-items: center; gap: 10px; padding: 6px 0 8px; font-size: 15px; }
.sheethead .serif { flex: 1; min-width: 0; font-weight: 600; }
.x { color: var(--t-ink-2); margin-right: -10px; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.page.stockpage { padding-bottom: calc(80px + env(safe-area-inset-bottom, 0px)); }
.page.stockpage.sub { padding-bottom: calc(var(--nav-h) + 24px + env(safe-area-inset-bottom, 0px)); }

@media (min-width: 1024px) {
  .page.stockpage { padding-bottom: 0; }
  .frame { display: grid; grid-template-columns: minmax(0, 1fr) 340px; align-items: start; max-width: calc(var(--container) + 64px); margin: 0 auto; padding: 0 32px; }
  .left { border-right: 1px solid var(--hair); position: sticky; top: var(--desk-header-h); height: calc(100vh - var(--desk-header-h)); }
  .rail0 { display: flex; justify-content: center; padding-top: 10px; }
  .cbtn { width: 26px; height: 26px; border: 0; background: transparent; padding: 0; color: var(--muted); display: grid; place-items: center; }
  .dscrim { align-items: stretch; justify-content: flex-start; }
  .drawer { width: 300px; background: var(--ground); height: 100%; padding-top: 8px; }
  .center { --gutter: 28px; min-width: 0; padding-bottom: 48px; }
  .right { position: sticky; top: var(--desk-header-h); max-height: calc(100vh - var(--desk-header-h)); overflow: auto; min-height: calc(100vh - var(--desk-header-h)); box-shadow: none; }
  .tsec { padding: 12px 16px; border-bottom: 1px solid var(--t-line); }
  .tsec.last { border-bottom: 0; }
  .ipodesk { --ink: var(--t-ink); --ink-2: var(--t-ink-2); --muted: var(--t-ink-2); --hair: var(--t-line); --grid: var(--t-line); color: var(--t-ink); }
  .hero { padding: 22px 28px 18px; align-items: flex-end; gap: 20px; border-bottom: 1px solid var(--hair); }
  .deskmeta { display: flex !important; gap: 10px; align-items: baseline; font-size: 13px; color: var(--ink-2); flex-wrap: wrap; }
  .star2 { margin-left: 6px; border: 1px solid var(--base-line); background: transparent; font-size: 12px; padding: 1px 8px; color: var(--ink-2); }
  .star2.on { color: var(--ground); background: var(--ink); border-color: var(--ink); }
  .ht h1 { font-size: 34px; line-height: 1.2; -webkit-line-clamp: 2; }
  .ja { font-size: 13px; }
  .price { display: flex !important; flex-direction: column; align-items: flex-end; gap: 2px; font-size: 16px; flex: none; }
  .price .big { font-size: 46px; line-height: 1; }
  .quote { padding: 10px 28px; border-bottom: 1px solid var(--hair); }
  .stats { grid-template-columns: repeat(9, minmax(0, 1fr)); gap: 12px; }
  .stats div { flex-direction: column; gap: 2px; }
  .stats dd { font-size: 14px; }
  .chartsec { border-top: 0; padding-top: 8px; }
  .legend.desk-only { display: inline-flex !important; }
  .lower { --stick: var(--desk-header-h); margin-top: 10px; }
  .panel { padding-top: 20px; min-height: calc(100vh - var(--desk-header-h) - 52px); }
}
@media (min-width: 1180px) {
  .frame { grid-template-columns: 248px minmax(0, 1fr) 340px; }
  .frame.collapsed { grid-template-columns: 36px minmax(0, 1fr) 340px; }
}
</style>
