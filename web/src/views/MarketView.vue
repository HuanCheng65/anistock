<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink } from 'vue-router'
import type { MarketStock } from '../api'
import BrandMark from '../components/BrandMark.vue'
import ChangePill from '../components/ChangePill.vue'
import Cover from '../components/Cover.vue'
import EmptyState from '../components/EmptyState.vue'
import Icon from '../components/Icon.vue'
import MarketClock from '../components/MarketClock.vue'
import SearchPanel from '../components/SearchPanel.vue'
import Sparkline from '../components/Sparkline.vue'
import StockRow from '../components/StockRow.vue'
import Tabs from '../components/Tabs.vue'
import WatchAside from '../components/WatchAside.vue'
import { DESK, useMedia } from '../composables/useMedia'
import { usePoll } from '../composables/usePoll'
import { big, dir, hm, localDay, longDate, money, pct, signed, when } from '../format'
import { useMarket } from '../stores/market'
import { useSession } from '../stores/session'

const market = useMarket()
const session = useSession()
const searching = ref(false)
const desk = useMedia(DESK)
const tab = ref('gain')
const showAll = ref(false)

const { refresh } = usePoll(() => market.refreshQuotes(), 5000)
usePoll(() => market.refreshTonight(), 300000)
usePoll(() => market.refreshNews(40), 30000)

const today = computed(() => session.status?.day ?? localDay())
const idx = computed(() => market.indices?.indices ?? [])
const main = computed(() => idx.value.find((i) => i.key === 'all') ?? null)
const subs = computed(() => idx.value.filter((i) => i.key !== 'all'))
const spark = computed(() => (main.value?.series ?? []).slice(-30).map((p) => p[1]))

const breadth = computed(() => {
  const s = market.stocks ?? []
  return {
    up: s.filter((x) => x.change > 1e-9).length,
    down: s.filter((x) => x.change < -1e-9).length,
    flat: s.filter((x) => Math.abs(x.change) <= 1e-9).length,
  }
})

const tabs = [
  { key: 'gain', label: '涨幅' },
  { key: 'loss', label: '跌幅' },
  { key: 'turnover', label: '成交额' },
  { key: 'heat', label: '热度' },
]
const sorters: Record<string, (a: MarketStock, b: MarketStock) => number> = {
  gain: (a, b) => b.change - a.change,
  loss: (a, b) => a.change - b.change,
  turnover: (a, b) => b.turnover - a.turnover,
  heat: (a, b) => b.week_revenue - a.week_revenue,
}
const ranked = computed(() => [...(market.stocks ?? [])].sort(sorters[tab.value]))
const rows = computed(() => (showAll.value ? ranked.value : ranked.value.slice(0, 6)))
const extra = (s: MarketStock) =>
  tab.value === 'turnover' ? big(s.turnover) : tab.value === 'heat' ? `营收 ${big(s.week_revenue)}` : ''

const tonight = computed(() =>
  (market.tonight ?? [])
    .map((a) => ({ a, s: market.byId.get(a.company_id) }))
    .filter((x) => x.s),
)

const lead = computed(() => market.news?.[0] ?? null)
const more = computed(() => (market.news ?? []).slice(1, 6))
</script>

<template>
  <main class="page market">
    <header class="pagehead phone-only brandhead">
      <BrandMark :size="28" />
      <span class="serif word">AniEx</span><span class="cn">安易</span>
      <MarketClock class="clock" />
      <button type="button" class="iconbtn" aria-label="搜索" @click="searching = true"><Icon name="search" :size="20" /></button>
    </header>

    <div v-if="searching" class="searchsheet phone-only">
      <div class="sbar">
        <SearchPanel autofocus class="sp" @close="searching = false" />
        <button type="button" class="linkbtn cancel" @click="searching = false">取消</button>
      </div>
    </div>

    <div class="frame">
      <WatchAside class="desk-only left" />

      <div class="center">
        <section class="indexblock gutter">
          <div class="date">{{ longDate(today) }}</div>
          <div class="headline">
            <div class="mainidx">
              <h1 class="serif">番剧综合指数</h1>
              <template v-if="main">
                <div class="num level">{{ money(main.value) }}</div>
                <div class="num delta" :class="dir(main.change)">{{ signed(main.value - main.prev) }}&nbsp;&nbsp;{{ pct(main.change) }}</div>
              </template>
              <div v-else class="skel" style="height: 60px; width: 180px"></div>
            </div>
            <Sparkline v-if="main" :values="spark" :base="spark[0]" :w="desk ? 280 : 132" :h="desk ? 80 : 56" class="spk" />
          </div>
          <div class="subs">
            <div v-for="i in subs" :key="i.key" class="sub">
              <span class="label">{{ i.name }}</span>
              <span class="num v">{{ money(i.value) }}&nbsp;&nbsp;<span :class="dir(i.change)">{{ pct(i.change) }}</span></span>
            </div>
          </div>
          <div v-if="market.stocks" class="breadth" role="img" :aria-label="`上涨 ${breadth.up} 支，平盘 ${breadth.flat} 支，下跌 ${breadth.down} 支`">
            <div class="bar">
              <span :style="{ flex: breadth.up }" class="b-up"></span>
              <span :style="{ flex: breadth.flat }" class="b-flat"></span>
              <span :style="{ flex: breadth.down }" class="b-down"></span>
            </div>
            <div class="num counts">
              <span class="up">↑ {{ breadth.up }}</span>
              <span class="muted">{{ breadth.flat }}</span>
              <span class="down">↓ {{ breadth.down }}</span>
            </div>
          </div>
        </section>

        <section v-if="tonight.length" class="section tonight">
          <h2 class="h2 gutter">今晚放送</h2>
          <div class="strip gutter">
            <RouterLink v-for="{ a, s } in tonight" :key="a.company_id" :to="`/stock/${a.company_id}`" class="air">
              <Cover :id="s!.id" :title="s!.title" :w="112" :h="150">
                <span class="num time" :class="{ done: a.aired }">{{ hm(a.airing_at) }}</span>
              </Cover>
              <span class="clamp2 t">{{ s!.title }}</span>
              <span class="num" :class="dir(s!.change)">{{ pct(s!.change) }}</span>
            </RouterLink>
          </div>
        </section>

        <section class="section ranks">
          <Tabs v-model="tab" :items="tabs" label="排行" class="gutter tabs" />
          <EmptyState v-if="market.error && !market.stocks" :text="market.error" action="重试" tone="error" @act="refresh" />
          <template v-else-if="market.stocks">
            <!-- phone: list rows -->
            <div class="phone-only">
              <StockRow v-for="s in rows" :key="s.id" :s="s" :extra="extra(s)" />
            </div>
            <!-- desktop: table -->
            <table class="table desk-only dtable">
              <thead>
                <tr><th>名称</th><th>现价</th><th>涨跌幅</th><th>成交额</th><th>本周营收</th><th>近 20 日</th></tr>
              </thead>
              <tbody>
                <tr v-for="s in rows" :key="s.id">
                  <td class="namecell">
                    <RouterLink :to="`/stock/${s.id}`" class="nm">
                      <Cover :id="s.id" :title="s.title" :w="24" :h="34" :spine="false" />
                      <span class="clamp1 serif">{{ s.title }}</span>
                      <span class="num muted code">{{ s.code }}</span>
                    </RouterLink>
                  </td>
                  <td class="num">{{ money(s.price) }}</td>
                  <td><ChangePill :value="s.change" /></td>
                  <td class="num">{{ big(s.turnover) }}</td>
                  <td class="num">{{ big(s.week_revenue) }}</td>
                  <td><Sparkline :values="s.spark" :base="s.spark[0]" :w="72" :h="22" class="mini" /></td>
                </tr>
              </tbody>
            </table>
            <button v-if="!showAll" type="button" class="all" @click="showAll = true">全部 {{ market.stocks.length }} 支</button>
          </template>
          <div v-else class="gutter">
            <div v-for="i in 6" :key="i" class="skel" style="height: 50px; margin: 10px 0"></div>
          </div>
        </section>

        <section class="section news gutter phone-only">
          <h2 class="h2">要闻</h2>
          <template v-if="lead">
            <RouterLink :to="lead.company_id ? `/stock/${lead.company_id}` : '/news'" class="lead">
              <span class="serif lt">{{ lead.title }}</span>
              <span class="muted small">{{ when(lead.ts ?? lead.day + 'T06:30:00+08:00', today) }}</span>
            </RouterLink>
            <div class="list">
              <RouterLink v-for="n in more" :key="n.id" :to="n.company_id ? `/stock/${n.company_id}` : '/news'" class="item">
                <span class="num tm">{{ n.ts ? hm(n.ts) : n.day.slice(5) }}</span>
                <span>{{ n.title }}</span>
              </RouterLink>
            </div>
          </template>
          <p v-else-if="market.news" class="muted small">暂无资讯</p>
        </section>
      </div>

      <aside class="desk-only right">
        <section class="news gutter">
          <h2 class="h2">要闻</h2>
          <template v-if="lead">
            <RouterLink :to="lead.company_id ? `/stock/${lead.company_id}` : '/news'" class="lead">
              <span class="serif lt">{{ lead.title }}</span>
              <span class="muted small">{{ when(lead.ts ?? lead.day + 'T06:30:00+08:00', today) }}</span>
            </RouterLink>
            <div class="list">
              <RouterLink v-for="n in more" :key="n.id" :to="n.company_id ? `/stock/${n.company_id}` : '/news'" class="item">
                <span class="num tm">{{ n.ts ? hm(n.ts) : n.day.slice(5) }}</span>
                <span>{{ n.title }}</span>
              </RouterLink>
            </div>
            <RouterLink to="/news" class="small ink2">全部资讯</RouterLink>
          </template>
          <p v-else-if="market.news" class="muted small">暂无资讯</p>
        </section>
      </aside>
    </div>
  </main>
</template>

<style scoped>
.brandhead { gap: 10px; }
.word { font-weight: 900; font-size: 19px; letter-spacing: 0.01em; }
.cn { font-size: 13px; color: var(--ink-2); }
.clock { margin-left: auto; }
.brandhead .iconbtn { width: 36px; height: 36px; }

.searchsheet { position: fixed; inset: 0; z-index: 50; background: var(--ground); overflow: auto; }
.sbar { display: flex; gap: 12px; align-items: flex-start; padding: calc(12px + env(safe-area-inset-top, 0px)) var(--gutter); }
.sp { flex: 1; min-width: 0; }
.cancel { color: var(--ink); font-size: 15px; height: 40px; }

.indexblock { padding-top: 18px; padding-bottom: 16px; display: flex; flex-direction: column; gap: 4px; }
.date { font-size: 12px; color: var(--muted); }
.headline { display: flex; align-items: flex-end; justify-content: space-between; gap: 12px; }
.mainidx { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.mainidx h1 { font-size: 17px; font-weight: 600; }
.level { font-size: 38px; font-weight: 600; line-height: 1.05; letter-spacing: -0.02em; }
.delta { font-size: 15px; font-weight: 500; }
.subs { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); margin-top: 14px; border-top: 1px solid var(--hair); }
.sub { padding: 10px 0; display: flex; flex-direction: column; gap: 2px; }
.sub:first-child { border-right: 1px solid var(--hair); }
.sub:nth-child(2) { padding-left: 14px; }
.label { font-size: 12px; color: var(--ink-2); }
.v { font-size: 15px; }
.breadth { display: flex; flex-direction: column; gap: 6px; margin-top: 4px; }
.bar { display: flex; height: 6px; gap: 2px; }
.b-up { background: var(--up); }
.b-flat { background: var(--base-line); }
.b-down { background: var(--down); }
.counts { display: flex; justify-content: space-between; font-size: 12px; }

.tonight { padding: 14px 0 16px; display: flex; flex-direction: column; gap: 12px; }
.strip { display: flex; gap: 12px; overflow-x: auto; scrollbar-width: none; }
.air { flex: none; width: 112px; display: flex; flex-direction: column; gap: 6px; font-size: 13px; color: var(--ink); }
.air .t { line-height: 1.35; height: 35px; }
.time { position: absolute; left: 8px; bottom: 10px; font-size: 11px; color: #fff; background: #17181C; padding: 2px 5px; }
.time.done { background: rgba(23, 24, 28, 0.6); }

.ranks { display: flex; flex-direction: column; }
.tabs { padding-top: 12px; }
.all { border: 0; background: transparent; padding: 14px 16px; font-size: 13px; color: var(--ink-2); }

.news { padding-top: 16px; padding-bottom: 16px; display: flex; flex-direction: column; gap: 14px; }
.lead { display: flex; flex-direction: column; gap: 6px; color: var(--ink); }
.lt { font-size: 21px; font-weight: 900; line-height: 1.35; }
.small { font-size: 12px; }
.list { display: flex; flex-direction: column; border-top: 1px solid var(--hair); }
.item { display: flex; gap: 12px; padding: 10px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; line-height: 1.45; color: var(--ink); }
.tm { flex: none; width: 40px; font-size: 12px; color: var(--muted); padding-top: 2px; }

@media (min-width: 1024px) {
  .frame { display: grid; grid-template-columns: 260px minmax(0, 1fr) 320px; align-items: start; }
  .left { border-right: 1px solid var(--hair); min-height: calc(100vh - var(--desk-header-h)); position: sticky; top: var(--desk-header-h); max-height: calc(100vh - var(--desk-header-h)); overflow: auto; }
  .center { --gutter: 28px; min-width: 0; }
  .right { border-left: 1px solid var(--hair); min-height: calc(100vh - var(--desk-header-h)); }
  .right .news { --gutter: 20px; }
  .indexblock { padding-top: 22px; }
  .level { font-size: 46px; }
  .subs { max-width: 520px; }
  .dtable { margin: 0 28px; width: calc(100% - 56px); }
  .dtable td { padding-top: 8px; padding-bottom: 8px; }
  .namecell { max-width: 0; width: 46%; }
  .nm { display: flex; align-items: center; gap: 10px; min-width: 0; color: var(--ink); }
  .nm .serif { font-size: 14px; font-weight: 600; min-width: 0; }
  .code { font-size: 11px; flex: none; }
  .mini { margin-left: auto; }
  .all { align-self: flex-start; padding-left: 28px; }
}
</style>
