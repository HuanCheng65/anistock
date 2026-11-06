<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
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
import RevenueBars from '../components/RevenueBars.vue'
import Tabs from '../components/Tabs.vue'
import TradeTape from '../components/TradeTape.vue'
import ListSidebar from '../components/ListSidebar.vue'
import { DESK, useMedia } from '../composables/useMedia'
import { usePoll } from '../composables/usePoll'
import { useStockNav } from '../composables/useStockNav'
import { big, bigMoney, dir, hm, money, pct, qty, share, shortDay, signed, weekdayTime } from '../format'
import { useAccount } from '../stores/account'
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
usePoll(() => (session.me ? Promise.all([account.refreshPositions(), account.refreshOrders()]) : null), 15000, 60000)
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
       { k: '市盈率(动)', v: s.value?.ratios.pe_dynamic != null ? s.value.ratios.pe_dynamic.toFixed(1) : '–', c: '', n: null },
       { k: '股息率(动)', v: s.value?.ratios.dividend_yield_dynamic != null ? share(s.value.ratios.dividend_yield_dynamic, 1) : '–', c: '', n: null }]
    : [...base, ...tail]
})
const periods = [
  { key: 'intraday', label: '分时' },
  { key: 'day', label: '日 K' },
  { key: 'week', label: '周 K' },
  { key: 'month', label: '月 K' },
]
const periodModel = computed({ get: () => period.value as string, set: (v: string) => (period.value = v as Period) })

const holders = computed(() => {
  const h = s.value?.holders
  const total = s.value?.shares || 1
  if (!h) return null
  return [
    { k: '制作委员会', v: h.committee / total, c: 'var(--hold-committee)' },
    { k: '做市商', v: h.market_maker / total, c: 'var(--hold-mm)' },
    { k: 'NPC', v: h.npc.total / total, c: 'var(--hold-npc)' },
    { k: '玩家', v: h.player / total, c: 'var(--hold-player)' },
    { k: '其他', v: h.other / total, c: 'var(--hold-unissued)' },
    { k: '待发行', v: h.unissued / total, c: 'var(--hold-unissued)' },
  ].filter((x) => x.v > 0 || x.k === '玩家')
})
const ratios = computed(() => {
  const r = s.value?.ratios
  return [
    { k: '市盈率(动)', v: r?.pe_dynamic != null ? r.pe_dynamic.toFixed(1) : '–' },
    { k: '市盈率(TTM)', v: r?.pe_ttm != null ? r.pe_ttm.toFixed(1) : '–' },
    { k: '股息率(动)', v: r?.dividend_yield_dynamic != null ? share(r.dividend_yield_dynamic, 1) : '–' },
    { k: '股息率(TTM)', v: r?.dividend_yield_ttm != null ? share(r.dividend_yield_ttm, 1) : '–' },
    { k: '市净率', v: r?.pb != null ? r.pb.toFixed(2) : '–' },
  ]
})
const events = computed(() => {
  const out: { d: string; t: string; key: string }[] = []
  for (const n of store.news[id.value] ?? []) out.push({ d: n.trading_day, t: n.title, key: 'n' + n.id })
  for (const e of s.value?.episode_days ?? []) out.push({ d: e.trading_day, t: e.episode === 1 && !s.value?.is_long_runner ? '首播' : `第 ${e.episode} 集播出`, key: 'e' + e.episode })
  for (const d of s.value?.dividends ?? []) out.push({ d: d.trading_day, t: `分红每股 ${money(d.per_share)}`, key: 'd' + d.trading_day })
  return out.sort((a, b) => (a.d < b.d ? 1 : a.d > b.d ? -1 : 0)).slice(0, 8)
})
// public audience numbers (collector snapshots); a source without data is left out
const audience = computed(() => {
  const a = s.value?.audiences
  if (!a) return []
  const out: { k: string; v: string }[] = []
  if (a.bangumi?.score != null) out.push({ k: 'Bangumi 评分', v: a.bangumi.score.toFixed(1) })
  if (a.bangumi?.collection) out.push({ k: 'Bangumi 在看', v: big(a.bangumi.collection.doing) })
  if (a.anilist?.score != null) out.push({ k: 'AniList 评分', v: String(a.anilist.score) })
  if (a.anilist?.popularity != null) out.push({ k: 'AniList 人气', v: big(a.anilist.popularity) })
  if (a.annict?.watchers != null) out.push({ k: 'Annict 观看', v: big(a.annict.watchers) })
  return out
})
const pos = computed(() => (session.me ? (account.positions ?? []).find((x) => x.stock.id === id.value) ?? null : null))
const subscribing = computed(() => s.value?.status === 'subscribing')
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

          <div class="cols">
            <section class="sec gutter">
              <h2 class="h2">营收</h2>
              <RevenueBars v-if="s.revenue_weeks.length" :weeks="s.revenue_weeks" :h="desk ? 110 : 96" />
              <p v-else class="muted small">暂无营收</p>
              <dl class="facts ratios">
                <div v-for="r in ratios" :key="r.k"><dt>{{ r.k }}</dt><dd>{{ r.v }}</dd></div>
              </dl>
            </section>
            <section class="sec gutter events">
              <h2 class="h2">动态</h2>
              <div v-for="e in events" :key="e.key" class="ev">
                <span class="num d">{{ shortDay(e.d, session.status?.trading_day) }}</span>
                <span>{{ e.t }}</span>
              </div>
              <p v-if="!events.length" class="muted small">暂无动态</p>
            </section>
          </div>

          <div class="cols">
            <section v-if="holders" class="sec gutter">
              <h2 class="h2">股东</h2>
              <div class="hbar" role="img" :aria-label="holders.map((h) => `${h.k} ${share(h.v)}`).join('，')">
                <span v-for="h in holders" :key="h.k" :style="{ flex: Math.max(h.v, 0.0001), background: h.c }"></span>
              </div>
              <div class="hleg">
                <span v-for="h in holders" :key="h.k">{{ h.k }} <span class="num ink">{{ share(h.v) }}</span></span>
              </div>
            </section>
            <section v-if="audience.length" class="sec gutter">
              <h2 class="h2">观众</h2>
              <dl class="facts">
                <div v-for="x in audience" :key="x.k"><dt>{{ x.k }}</dt><dd>{{ x.v }}</dd></div>
              </dl>
            </section>
            <section v-if="pos" class="sec gutter">
              <h2 class="h2">持仓</h2>
              <dl class="facts">
                <div><dt>数量</dt><dd>{{ qty(pos.quantity) }}</dd></div>
                <div><dt>成本</dt><dd>{{ money(pos.average_cost) }}</dd></div>
                <div><dt>市值</dt><dd>{{ money(pos.value) }}</dd></div>
                <div><dt>盈亏</dt><dd :class="dir(pos.pnl)">{{ signed(pos.pnl) }}</dd></div>
                <div><dt>盈亏比例</dt><dd :class="dir(pos.pnl)">{{ pct(pos.pnl_ratio) }}</dd></div>
                <div><dt>累计分红</dt><dd>{{ money(pos.dividends) }}</dd></div>
              </dl>
            </section>
          </div>
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
.cols .sec + .sec { margin-top: 0; }
.ratios { border-top: 1px solid var(--hair); margin-top: 4px; }
.events { gap: 0; }
.events .h2 { margin-bottom: 6px; }
.ev { display: flex; gap: 12px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; line-height: 1.45; }
.ev .d { flex: none; width: 44px; font-size: 12px; color: var(--muted); padding-top: 2px; }
.small { font-size: 13px; }
.hbar { display: flex; height: 10px; gap: 2px; }
.hleg { display: flex; flex-wrap: wrap; gap: 4px 16px; font-size: 12px; color: var(--ink-2); }
.ink { color: var(--ink); }

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
  .cols { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); border-top: 1px solid var(--rule); margin-top: 6px; }
  .cols .sec { border-top: 0; margin-top: 0; padding-top: 16px; padding-bottom: 16px; }
  .cols .sec:first-child { border-right: 1px solid var(--hair); }
}
@media (min-width: 1180px) {
  .frame { grid-template-columns: 248px minmax(0, 1fr) 340px; }
  .frame.collapsed { grid-template-columns: 36px minmax(0, 1fr) 340px; }
}
</style>
