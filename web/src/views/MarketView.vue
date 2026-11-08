<script setup lang="ts">
// 市场: overview modules (总览 frame). Desktop: a 12-column module grid; phone: the same modules stacked.
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import type { Quote } from '../api'
import BrandLockup from '../components/BrandLockup.vue'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import MarketIntro from '../components/MarketIntro.vue'
import MarketClock from '../components/MarketClock.vue'
import SearchPanel from '../components/SearchPanel.vue'
import StartSteps from '../components/StartSteps.vue'
import Sparkline from '../components/Sparkline.vue'
import Tabs from '../components/Tabs.vue'
import { DESK, useMedia } from '../composables/useMedia'
import { usePoll } from '../composables/usePoll'
import { bigMoney, dir, hm, longDate, money, num, pct, signedNum, tradingDay } from '../format'
import { useIpo } from '../stores/ipo'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

const market = useMarket()
const session = useSession()
const prefs = usePrefs()
const ipo = useIpo()
const desk = useMedia(DESK)
const searching = ref(false)
const rankTab = ref('gain')

const { refresh } = usePoll(() => market.refreshQuotes(), 5000, 30000)
usePoll(() => market.refreshTonight(), 300000)
usePoll(() => market.refreshNews(40), 30000, 120000)
usePoll(() => market.refreshMacro(), 300000)
usePoll(() => Promise.all([session.loadLeaders('all'), ipo.refresh()]), 60000)

const today = computed(() => session.status?.trading_day ?? tradingDay())
const idx = computed(() => market.indices?.items ?? [])
const main = computed(() => idx.value.find((i) => i.key === 'all') ?? null)
const subs = computed(() => idx.value.filter((i) => i.key !== 'all'))
const spark = computed(() => (main.value?.series ?? []).slice(-30).map((p) => p.value))
const MOOD = { cold: '冷淡', neutral: '平稳', hot: '火热' } as const
const mood = computed(() => {
  const s = session.status?.sentiment
  return s ? MOOD[s] : null
})
const breadth = computed(() => {
  const s = market.stocks ?? []
  return {
    up: s.filter((x) => x.change > 5e-5).length,
    down: s.filter((x) => x.change < -5e-5).length,
    flat: s.filter((x) => Math.abs(x.change) <= 5e-5).length,
  }
})

const watch = computed(() => prefs.watchlist.map((id) => market.byId.get(id)).filter((s): s is Quote => !!s))
const by = (f: (a: Quote, b: Quote) => number) => [...(market.stocks ?? [])].sort(f).slice(0, 6)
const lists = computed(() => ({
  gain: { label: '涨幅', rows: by((a, b) => b.change - a.change), link: '/stocks?s=change&d=desc' },
  loss: { label: '跌幅', rows: by((a, b) => a.change - b.change), link: '/stocks?s=change&d=asc' },
  turnover: { label: '成交额', rows: by((a, b) => b.turnover - a.turnover), link: '/stocks?s=turnover&d=desc' },
}))
const rankTabs = [
  { key: 'gain', label: '涨幅' },
  { key: 'loss', label: '跌幅' },
  { key: 'turnover', label: '成交额' },
]
const tonight = computed(() =>
  (market.tonight ?? []).map((a) => ({ a, s: market.byId.get(a.stock.id) })).filter((x) => x.s).slice(0, 8),
)
const news = computed(() => (market.news ?? []).slice(0, desk.value ? 5 : 4))

const macroRows = computed(() => {
  const rows = market.macro ?? []
  const series = (f: (r: (typeof rows)[number]) => number | null) => rows.map(f).filter((v): v is number => v != null).slice(-30)
  const last = <T,>(a: T[]) => a[a.length - 1]
  const demand = series((r) => r.demand_index)
  const m2 = series((r) => r.m2)
  const rev = series((r) => r.revenue_7d)
  return [
    { k: '基准利率', series: series((r) => r.policy_rate), v: session.status ? (session.status.policy_rate * 100).toFixed(2) + '%' : '–' },
    { k: '外部需求指数', series: demand, v: demand.length ? last(demand).toFixed(2) : '–' },
    { k: '货币总量', series: m2, v: m2.length ? bigMoney(last(m2)) : '–' },
    { k: '本周营收合计', series: rev, v: rev.length ? bigMoney(last(rev)) : '–' },
  ]
})
const board = computed(() => session.boards.all ?? null)
const leaders = computed(() => (board.value?.items ?? []).slice(0, 3))
const myRank = computed(() => {
  const r = board.value?.items.find((x) => x.user.id === session.me?.id)
  return r && r.rank > 3 ? r : null
})
const offer = computed(() => ipo.open[0] ?? null)
const offerHeat = computed(() => (offer.value && offer.value.base_public_quantity ? offer.value.applied_quantity / offer.value.base_public_quantity : 0))
const recent = computed(() => ipo.listed.map((l) => market.byId.get(l.stock.id)).filter((s): s is Quote => !!s).slice(0, 2))

function remember(key: string, label: string, rows: Quote[]) {
  prefs.context = { key, label, ids: rows.map((s) => s.stock.id) }
}
</script>

<template>
  <main class="page market">
    <header class="pagehead phone-only brandhead">
      <BrandLockup :height="22" />
      <MarketClock class="clock" />
      <button type="button" class="iconbtn sbtn" aria-label="搜索" @click="searching = true"><Icon name="search" :size="20" /></button>
    </header>
    <div v-if="searching" class="searchsheet phone-only">
      <div class="sbar">
        <SearchPanel autofocus class="sp" @close="searching = false" />
        <button type="button" class="linkbtn cancel" @click="searching = false">取消</button>
      </div>
    </div>

    <div v-if="session.checked && !session.me" class="container"><MarketIntro /></div>
    <div v-else-if="session.me" class="container"><StartSteps /></div>
    <div class="container grid">
      <!-- 指数 -->
      <section class="mod m-index">
        <div class="modhead desk-only"><h2>指数</h2></div>
        <div class="date phone-only">{{ longDate(today) }}</div>
        <div class="headline">
          <div class="mainidx">
            <span class="lbl">番剧综合指数</span>
            <template v-if="main">
              <span class="num level">{{ num(main.value) }}</span>
              <span class="num delta" :class="dir(main.change)">{{ signedNum(main.value - main.prev_close) }}&nbsp;&nbsp;{{ pct(main.change) }}</span>
            </template>
            <span v-else class="skel" style="height: 56px; width: 180px; display: block"></span>
          </div>
          <Sparkline v-if="main" :values="spark" :base="spark[0]" :w="desk ? 340 : 132" :h="desk ? 84 : 56" class="spk" />
        </div>
        <div class="subs">
          <div v-for="i in subs" :key="i.key" class="sub">
            <span class="lbl">{{ i.name }}</span>
            <span class="num v">{{ num(i.value) }}&nbsp;&nbsp;<span :class="dir(i.change)">{{ pct(i.change) }}</span></span>
          </div>
          <div v-if="mood" class="sub mood">
            <span class="lbl">市场情绪</span>
            <span class="v">{{ mood }}</span>
          </div>
        </div>
        <div v-if="market.stocks" class="breadth" role="img" :aria-label="`上涨 ${breadth.up} 支，平盘 ${breadth.flat} 支，下跌 ${breadth.down} 支`">
          <div class="bar">
            <span :style="{ flex: breadth.up }" class="b-up"></span>
            <span :style="{ flex: breadth.flat }" class="b-flat"></span>
            <span :style="{ flex: breadth.down }" class="b-down"></span>
          </div>
          <div class="num counts"><span class="up">↑ {{ breadth.up }}</span><span class="muted">{{ breadth.flat }}</span><span class="down">↓ {{ breadth.down }}</span></div>
        </div>
      </section>

      <!-- 自选 -->
      <section class="mod m-watch" :class="{ 'phone-hide': !watch.length }">
        <div class="modhead"><h2>自选</h2><RouterLink to="/stocks?f=watch">全部自选</RouterLink></div>
        <RouterLink v-for="s in watch.slice(0, 6)" :key="s.stock.id" :to="`/stocks/${s.stock.id}`" class="mrow" @click="remember('watch', '自选', watch)">
          <Cover :id="s.stock.id" :title="s.stock.title" :w="22" :h="22" :spine="false" />
          <span class="clamp1">{{ s.stock.title }}</span>
          <span v-flash="s.price" class="num">{{ money(s.price) }}</span>
          <span v-flash="s.change" class="num" :class="dir(s.change)">{{ pct(s.change) }}</span>
        </RouterLink>
        <p v-if="!watch.length" class="none">在股票页加入自选</p>
      </section>

      <!-- 要闻 -->
      <section class="mod m-news">
        <div class="modhead"><h2>要闻</h2><RouterLink to="/news">全部资讯</RouterLink></div>
        <RouterLink v-for="n in news" :key="n.id" :to="n.stock ? `/stocks/${n.stock.id}` : '/news'" class="nrow" :class="{ fresh: market.fresh.has(n.id) }">
          <span class="num tm">{{ hm(n.at) }}</span>
          <span>{{ n.title }}</span>
        </RouterLink>
        <p v-if="market.news && !news.length" class="none">暂无资讯</p>
      </section>

      <!-- 今晚放送 -->
      <section v-if="tonight.length" class="mod m-tonight">
        <div class="modhead"><h2>今晚放送</h2></div>
        <div class="strip">
          <RouterLink v-for="{ a, s } in tonight" :key="a.stock.id + '-' + a.episode" :to="`/stocks/${a.stock.id}`" class="air">
            <Cover :id="s!.stock.id" :title="s!.stock.title" :w="112" :h="150" class="ac">
              <span class="num time" :class="{ done: a.has_aired }">{{ hm(a.airs_at) }}</span>
            </Cover>
            <span class="clamp1 t">{{ s!.stock.title }}</span>
            <span v-flash="s!.change" class="num" :class="dir(s!.change)">{{ pct(s!.change) }}</span>
          </RouterLink>
        </div>
      </section>

      <!-- 涨幅 / 跌幅 / 成交额 -->
      <EmptyState v-if="market.error && !market.stocks" class="m-wide" :text="market.error" action="重试" tone="error" @act="refresh" />
      <template v-else>
        <section v-for="(l, k) in lists" :key="k" class="mod m-list desk-only">
          <div class="modhead"><h2>{{ l.label }}</h2><RouterLink :to="l.link">全部股票</RouterLink></div>
          <RouterLink v-for="s in l.rows" :key="s.stock.id" :to="`/stocks/${s.stock.id}`" class="mrow" @click="remember('all', '全部', l.rows)">
            <Cover :id="s.stock.id" :title="s.stock.title" :w="22" :h="22" :spine="false" />
            <span class="clamp1">{{ s.stock.title }}</span>
            <span v-flash="k === 'turnover' ? s.turnover : s.price" class="num">{{ k === 'turnover' ? bigMoney(s.turnover) : money(s.price) }}</span>
            <span v-flash="s.change" class="num" :class="dir(s.change)">{{ pct(s.change) }}</span>
          </RouterLink>
        </section>
        <section class="mod m-wide phone-only">
          <Tabs v-model="rankTab" :items="rankTabs" label="排行" />
          <RouterLink v-for="s in lists[rankTab as 'gain'].rows" :key="s.stock.id" :to="`/stocks/${s.stock.id}`" class="mrow" @click="remember('all', '全部', lists[rankTab as 'gain'].rows)">
            <Cover :id="s.stock.id" :title="s.stock.title" :w="22" :h="22" :spine="false" />
            <span class="clamp1">{{ s.stock.title }}</span>
            <span v-flash="rankTab === 'turnover' ? s.turnover : s.price" class="num">{{ rankTab === 'turnover' ? bigMoney(s.turnover) : money(s.price) }}</span>
            <span v-flash="s.change" class="num" :class="dir(s.change)">{{ pct(s.change) }}</span>
          </RouterLink>
          <RouterLink :to="lists[rankTab as 'gain'].link" class="more">全部 {{ market.stocks?.length ?? '' }} 支</RouterLink>
        </section>
      </template>

      <!-- 宏观 -->
      <section class="mod m-third">
        <div class="modhead"><h2>宏观</h2><RouterLink to="/macro">详情</RouterLink></div>
        <RouterLink v-for="r in macroRows" :key="r.k" to="/macro" class="krow">
          <span>{{ r.k }}</span>
          <Sparkline v-if="r.series.length > 1" :values="r.series" :base="r.series[0]" :w="64" :h="20" stroke="var(--ink-2)" />
          <span v-else></span>
          <span class="num">{{ r.v }}</span>
        </RouterLink>
      </section>

      <!-- 排行 -->
      <section class="mod m-third">
        <div class="modhead"><h2>排行</h2><RouterLink to="/rank">完整排行</RouterLink></div>
        <div v-for="r in leaders" :key="r.user.id" class="rrow" :class="{ me: r.user.id === session.me?.id }">
          <span class="serif rk" :class="{ first: r.rank === 1 }">{{ r.rank }}</span>
          <span class="clamp1">{{ r.user.username }}<template v-if="r.user.id === session.me?.id">（我）</template></span>
          <span class="num" :class="dir(r.return)">{{ pct(r.return) }}</span>
        </div>
        <div v-if="myRank" class="rrow me">
          <span class="serif rk">{{ myRank.rank }}</span>
          <span class="clamp1">{{ myRank.user.username }}（我）</span>
          <span class="num" :class="dir(myRank.return)">{{ pct(myRank.return) }}</span>
        </div>
        <p v-if="board && !leaders.length" class="none">暂无玩家</p>
      </section>

      <!-- 新股 -->
      <section class="mod m-third">
        <div class="modhead"><h2>新股</h2><RouterLink to="/ipo">新股认购</RouterLink></div>
        <RouterLink v-if="offer" to="/ipo" class="offer">
          <Cover :id="offer.stock.id" :title="offer.stock.title" :w="64" :h="90" :size="13" />
          <span class="oi">
            <span class="serif ot">《{{ offer.stock.title }}》</span>
            <span class="om">认购中 · {{ offer.sub_end.slice(5) }} 截止</span>
            <span class="om">发行价 <span class="num">{{ money(offer.price) }}</span> · 超额认购 <span class="num">{{ offerHeat.toFixed(1) }}</span> 倍</span>
          </span>
        </RouterLink>
        <p v-else-if="ipo.ipos" class="none">暂无认购中的新股</p>
        <RouterLink v-for="s in recent" :key="s.stock.id" :to="`/stocks/${s.stock.id}`" class="mrow">
          <Cover :id="s.stock.id" :title="s.stock.title" :w="22" :h="22" :spine="false" />
          <span class="clamp1">{{ s.stock.title }}</span>
          <span v-flash="s.price" class="num">{{ money(s.price) }}</span>
          <span v-flash="s.change" class="num" :class="dir(s.change)">{{ pct(s.change) }}</span>
        </RouterLink>
      </section>
    </div>
  </main>
</template>

<style scoped>
.brandhead { gap: 10px; }
.clock { margin-left: auto; }
.sbtn { width: 36px; height: 36px; }
.searchsheet { position: fixed; inset: 0; z-index: 50; background: var(--ground); overflow: auto; }
.sbar { display: flex; gap: 12px; align-items: flex-start; padding: calc(12px + env(safe-area-inset-top, 0px)) 16px; }
.sp { flex: 1; min-width: 0; }
.cancel { color: var(--ink); font-size: 15px; height: 40px; }

.grid { display: flex; flex-direction: column; }
.m-index { order: 1; }
.m-watch { order: 2; }
.m-tonight { order: 3; }
.m-wide { order: 4; }
.m-news { order: 5; }
.m-third { order: 6; }
.mod.m-index + .mod, .mod { border-top: 1px solid var(--rule); }
.mod.m-index { border-top: 0; }
.mod { padding-top: 18px; padding-bottom: 6px; min-width: 0; }
.mod .modhead { border-bottom-color: var(--hair); }
.none { padding: 14px 0; font-size: 13px; color: var(--muted); }
.phone-hide { display: none; }

.date { font-size: 12px; color: var(--muted); }
.headline { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px; padding-top: 6px; }
.mainidx { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.lbl { font-size: 12px; color: var(--ink-2); }
.level { font-size: 38px; font-weight: 600; line-height: 1.05; letter-spacing: -0.02em; }
.delta { font-size: 15px; font-weight: 500; }
.subs { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); margin-top: 14px; border-top: 1px solid var(--hair); }
.sub { padding: 10px 0; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.sub:nth-child(2) { padding-left: 14px; border-left: 1px solid var(--hair); }
.sub.mood { grid-column: 1 / -1; flex-direction: row; justify-content: space-between; align-items: baseline; padding-left: 0; border-left: 0; border-top: 1px solid var(--hair); }
.v { font-size: 15px; }
.breadth { display: flex; flex-direction: column; gap: 6px; margin: 4px 0 10px; }
.bar { display: flex; height: 6px; gap: 2px; }
.b-up { background: var(--up); }
.b-flat { background: var(--base-line); }
.b-down { background: var(--down); }
.counts { display: flex; justify-content: space-between; font-size: 12px; }

.nrow { display: grid; grid-template-columns: 44px minmax(0, 1fr); gap: 10px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; line-height: 1.45; color: var(--ink); }
.tm { font-size: 12px; color: var(--muted); padding-top: 2px; }

.strip { display: flex; gap: 12px; overflow-x: auto; scrollbar-width: none; padding: 14px 0 8px; }
.air { flex: none; width: 112px; display: flex; flex-direction: column; gap: 6px; font-size: 13px; color: var(--ink); min-width: 0; }
.time { position: absolute; left: 8px; bottom: 10px; font-size: 11px; color: #fff; background: #17181C; padding: 2px 5px; }
.time.done { background: rgba(23, 24, 28, 0.6); }
.more { display: block; text-align: center; padding: 14px 0; font-size: 13px; color: var(--ink-2); }

.krow { display: grid; grid-template-columns: minmax(0, 1fr) 64px 84px; gap: 12px; align-items: center; padding: 11px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; color: var(--ink); }
.krow .num { text-align: right; font-size: 13px; }
.rrow { display: grid; grid-template-columns: 26px minmax(0, 1fr) auto; gap: 10px; align-items: baseline; padding: 11px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
.rrow .num { font-size: 13px; }
.rk { font-weight: 900; font-size: 15px; }
.rk.first { color: var(--ink); font-weight: 700; }
.rrow.me { font-weight: 700; }
.offer { display: flex; gap: 14px; padding: 14px 0; border-bottom: 1px solid var(--hair-2); color: var(--ink); }
.oi { display: flex; flex-direction: column; gap: 6px; min-width: 0; }
.ot { font-size: 17px; font-weight: 900; }
.om { font-size: 12px; color: var(--ink-2); }

@media (min-width: 1024px) {
  .grid { display: grid; grid-template-columns: repeat(12, minmax(0, 1fr)); column-gap: 32px; row-gap: 8px; padding-top: 28px; padding-bottom: 32px; }
  .mod { padding-top: 0; padding-bottom: 20px; }
  .mod, .mod.m-index { border-top: 0; order: 0; }
  .mod .modhead { border-bottom-color: var(--rule); }
  .phone-hide { display: block; }
  .m-index { grid-column: span 5; }
  .subs { grid-template-columns: none; grid-auto-flow: column; grid-auto-columns: minmax(0, 1fr); }
  .sub.mood { grid-column: auto; flex-direction: column; justify-content: flex-start; align-items: stretch; padding-left: 14px; border-left: 1px solid var(--hair); border-top: 0; }
  .m-index .modhead { margin-bottom: 14px; }
  .m-watch { grid-column: span 4; }
  .m-news { grid-column: span 3; }
  .m-tonight, .m-wide { grid-column: span 12; }
  .m-list, .m-third { grid-column: span 4; }
  .m-list.desk-only { display: block !important; }
  .strip { display: grid; grid-template-columns: repeat(8, minmax(0, 1fr)); gap: 16px; overflow: visible; }
  .air { width: auto; }
  .ac { width: 100% !important; height: auto !important; aspect-ratio: 156 / 150; }
}
.nrow.fresh { animation: news-in 0.35s ease-out, news-hl 1.6s ease-out; }
@keyframes news-in { from { transform: translateY(-8px); opacity: 0; } }
@keyframes news-hl { from { background-color: var(--flash-neutral); } }
@media (prefers-reduced-motion: reduce) { .nrow.fresh { animation: news-hl 1.2s steps(1, end); } }
</style>
