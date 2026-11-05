<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, ApiError, type Candle, type StockDetail } from '../api'
import CandleChart from '../components/CandleChart.vue'
import OrderBook from '../components/OrderBook.vue'
import OrderPanel from '../components/OrderPanel.vue'
import RevenueBars from '../components/RevenueBars.vue'
import IpoCard from '../components/IpoCard.vue'
import { useSession } from '../stores/session'
import { usePoll } from '../composables/usePoll'
import { airingIn, big, dir, episodes, hhmm, money, NEWS_KIND, pct, shortDate, signed, time } from '../format'

const route = useRoute()
const session = useSession()
const id = computed(() => Number(route.params.id))
const s = ref<StockDetail | null>(null)
const candles = ref<Candle[]>([])
const error = ref('')
const pick = ref<{ price: number; n: number } | null>(null)
const cancelling = ref<number | null>(null)
const actionMsg = ref('')
let candleAt = 0

async function load() {
  try {
    s.value = await api.get<StockDetail>(`/api/stocks/${id.value}`)
    error.value = ''
    if (Date.now() - candleAt > 30000 || !candles.value.length) {
      candles.value = (await api.get<{ candles: Candle[] }>(`/api/stocks/${id.value}/candles`)).candles
      candleAt = Date.now()
    }
  } catch (e) {
    error.value = e instanceof ApiError ? (e.status === 404 ? '没有这支股票' : e.message) : '加载失败'
  }
}
const { refresh } = usePoll(load, 5000)

async function afterOrder() {
  candleAt = 0
  await refresh()
}

async function cancel(orderId: number) {
  cancelling.value = orderId
  actionMsg.value = ''
  try {
    await api.del(`/api/orders/${orderId}`)
    await refresh()
  } catch (e) {
    actionMsg.value = e instanceof ApiError ? e.message : '撤单失败'
  } finally {
    cancelling.value = null
  }
}

function onPick(price: number) {
  pick.value = { price, n: (pick.value?.n ?? 0) + 1 }
}

const q = computed(() => s.value?.quote)
const change = computed(() => (q.value && q.value.prev_close ? (q.value.price - q.value.prev_close) / q.value.prev_close : 0))
const aired = computed(() => s.value?.weeks.length ? Math.max(...s.value.weeks.map((w) => w.aired)) : 0)
const dividends = computed(() => (s.value?.weeks ?? []).filter((w) => w.dividend_ps > 0).slice().reverse())
const lastDividend = computed(() => dividends.value[0] ?? null)
const holders = computed(() => {
  const h = s.value?.holders ?? {}
  const total = s.value?.shares || 1
  const players = (h.player ?? 0) + (h.noise ?? 0)
  return {
    committee: (h.committee ?? 0) / total,
    mm: (h.mm ?? 0) / total,
    players: players / total,
    unsold: (h.company ?? 0) / total,
  }
})
const bgm = computed(() => s.value?.audiences.bangumi)
const ani = computed(() => s.value?.audiences.anilist)
const annict = computed(() => s.value?.audiences.annict)
const subscribing = computed(() => s.value?.status === 'subscribing')
const vcm = computed(() => (s.value?.vcm?.active ? s.value.vcm : null))
const auction = computed(() => (session.status?.phase === 'auction' ? s.value?.auction ?? null : null))
</script>

<template>
  <main class="page stock">
    <RouterLink to="/" class="small back">← 行情</RouterLink>
    <p v-if="error" class="notice error">{{ error }}</p>

    <template v-if="s && q">
      <!-- quote strip -->
      <section class="panel quote">
        <div class="qtitle">
          <h1>{{ s.title }}</h1>
          <p v-if="s.title_ja && s.title_ja !== s.title" class="ja small muted">{{ s.title_ja }}</p>
          <div class="meta small">
            <span class="num muted">{{ s.code }}</span>
            <span v-if="subscribing" class="tag ipo">认购中</span>
            <span v-if="vcm" class="tag new">冷静期</span>
            <span v-if="s.long_runner" class="tag long">长篇</span>
            <span v-if="s.status !== 'listed'" class="tag long">已退市</span>
            <span class="muted" v-if="s.studio">{{ s.studio }}</span>
            <span class="muted" v-if="s.title_en">{{ s.title_en }}</span>
          </div>
        </div>
        <div class="qprice">
          <span class="price num" :class="dir(change)">{{ money(q.price) }}</span>
          <span class="num" :class="dir(change)">{{ signed(q.price - q.prev_close) }}　{{ pct(change) }}</span>
        </div>
        <dl class="qstats small">
          <div><dt>今开</dt><dd class="num">{{ money(q.open) }}</dd></div>
          <div><dt>最高</dt><dd class="num" :class="q.high != null ? dir(q.high - q.prev_close) : ''">{{ money(q.high) }}</dd></div>
          <div><dt>最低</dt><dd class="num" :class="q.low != null ? dir(q.low - q.prev_close) : ''">{{ money(q.low) }}</dd></div>
          <div><dt>昨收</dt><dd class="num">{{ money(q.prev_close) }}</dd></div>
          <div><dt>开盘参考</dt><dd class="num">{{ money(q.open_ref) }}</dd></div>
          <div><dt>今日冷静期</dt><dd class="num">{{ s.vcm?.count ?? 0 }} 次</dd></div>
          <div><dt>成交量</dt><dd class="num">{{ q.volume }} 股</dd></div>
          <div><dt>成交额</dt><dd class="num">{{ big(q.turnover) }}</dd></div>
        </dl>
        <p v-if="vcm" class="banner vcm" role="status">
          <b>冷静期 · 至 {{ hhmm(vcm.until) }}</b>
          <span>可成交区间 <b class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</b>。价格短时间内波动过大，暂时只允许在这个区间里成交。</span>
        </p>
        <p v-if="auction" class="banner auction" role="status">
          <b>集合竞价中</b>
          <span v-if="auction.price != null">参考开盘价 <b class="num">{{ money(auction.price) }}</b> · 可成交 <b class="num">{{ auction.volume }}</b> 股 · {{ session.status?.open }} 统一撮合</span>
          <span v-else>暂时没有可成交的买卖单；{{ session.status?.open }} 时若仍无成交，以昨收为开盘价</span>
        </p>
        <p v-if="subscribing" class="banner auction">
          <b>新股认购中</b><span>还没上市交易。认购截止当天日结时分配，次日上市。</span>
        </p>
      </section>

      <div class="cols">
        <div class="col main">
          <section class="panel">
            <div class="panel-head"><h2>日 K</h2><span class="small muted">上市 {{ s.listed_on }} · 发行价 {{ money(s.ipo_price) }}</span></div>
            <div class="panel-body"><CandleChart :candles="candles" /></div>
          </section>

          <section class="panel">
            <div class="panel-head"><h2>基本面</h2><span class="small muted">营收来自真实观众热度</span></div>
            <div class="panel-body fund">
              <RevenueBars v-if="s.weeks.length" :weeks="s.weeks" />
              <p v-else class="empty">还没有营收记录</p>
              <dl class="facts small">
                <div><dt>播出</dt><dd>{{ episodes(aired, s.episodes, s.long_runner) }}</dd></div>
                <div><dt>首播</dt><dd class="num">{{ s.first_air }}</dd></div>
                <div v-if="ani?.next_episode"><dt>下一集</dt><dd>第 {{ ani.next_episode.episode }} 集 · {{ airingIn(ani.next_episode.airingAt) }}</dd></div>
                <div><dt>每股净资产</dt><dd class="num">{{ money(s.book_value_ps) }}</dd></div>
                <div><dt>最近分红</dt><dd class="num">{{ lastDividend ? `${money(lastDividend.dividend_ps)}（${shortDate(lastDividend.week)} 那周）` : '暂无' }}</dd></div>
                <div><dt>总股本</dt><dd class="num">{{ s.shares.toLocaleString('zh-CN') }} 股</dd></div>
              </dl>
              <div class="holders small">
                <span class="label">持股结构</span>
                <div class="hbar" role="img" :aria-label="`制作委员会 ${pct(holders.committee, 0, false)}，流通 ${pct(1 - holders.committee, 0, false)}`">
                  <span :style="{ width: holders.committee * 100 + '%', background: 'var(--rule-strong)' }"></span>
                  <span :style="{ width: holders.players * 100 + '%', background: 'var(--accent)' }"></span>
                  <span :style="{ width: holders.mm * 100 + '%', background: 'var(--s-jp)' }"></span>
                  <span :style="{ width: holders.unsold * 100 + '%', background: 'var(--seal)' }"></span>
                </div>
                <div class="hlegend">
                  <span><i style="background: var(--rule-strong)"></i>制作委员会 {{ pct(holders.committee, 0, false) }}</span>
                  <span><i style="background: var(--accent)"></i>玩家与散户 {{ pct(holders.players, 0, false) }}</span>
                  <span><i style="background: var(--s-jp)"></i>做市商 {{ pct(holders.mm, 0, false) }}</span>
                  <span v-if="holders.unsold"><i style="background: var(--seal)"></i>待发行 {{ pct(holders.unsold, 0, false) }}</span>
                </div>
                <p class="muted">制作委员会代表真实世界的版权方，它分到的红利流出游戏；股价远高于价值时它会减持。</p>
              </div>
              <div class="aud small">
                <span class="label">观众规模（三个市场的数据源）</span>
                <div class="audgrid">
                  <div>
                    <span class="mk" style="color: var(--s-us)">欧美 · AniList</span>
                    <span class="num">{{ ani?.popularity != null ? big(ani.popularity) + ' 人收藏' : '–' }}</span>
                    <span class="muted" v-if="ani?.score">评分 {{ ani.score }}</span>
                  </div>
                  <div>
                    <span class="mk" style="color: var(--s-cn)">中国 · Bangumi</span>
                    <template v-if="bgm?.collection">
                      <span class="num">在看 {{ big(bgm.collection.doing ?? 0) }} · 想看 {{ big(bgm.collection.wish ?? 0) }}</span>
                      <span class="muted">看过 {{ big(bgm.collection.collect ?? 0) }} · 抛弃 {{ big(bgm.collection.dropped ?? 0) }}<template v-if="bgm.score"> · 评分 {{ bgm.score }}</template></span>
                    </template>
                    <span v-else class="muted">没有这部番的数据</span>
                  </div>
                  <div>
                    <span class="mk" style="color: var(--s-jp)">日本 · Annict</span>
                    <span class="num">{{ annict?.watchers != null ? big(annict.watchers) + ' 人关注' : '–' }}</span>
                  </div>
                </div>
              </div>
              <div v-if="dividends.length" class="divs small">
                <span class="label">分红记录（每股）</span>
                <div class="divlist">
                  <span v-for="d in dividends.slice(0, 12)" :key="d.week" class="num">{{ shortDate(d.week) }} <b>{{ money(d.dividend_ps) }}</b></span>
                </div>
              </div>
            </div>
          </section>

          <section class="panel" v-if="s.news.length">
            <div class="panel-head"><h2>公司公告</h2></div>
            <ul class="news">
              <li v-for="n in s.news" :key="n.id">
                <span class="small muted">{{ n.day }} · {{ NEWS_KIND[n.kind] ?? n.kind }}</span>
                <b>{{ n.title }}</b>
                <span v-if="n.body" class="small muted">{{ n.body }}</span>
              </li>
            </ul>
          </section>
        </div>

        <div class="col side">
          <section class="panel" v-if="subscribing">
            <div class="panel-head"><h2>认购</h2><RouterLink to="/ipo" class="small">全部新股 →</RouterLink></div>
            <div class="panel-body">
              <IpoCard v-if="s.ipo" :ipo="s.ipo" compact @changed="refresh" />
              <p v-else class="empty">认购信息暂时拿不到，请到“新股”页查看</p>
            </div>
          </section>
          <section class="panel trade" v-else>
            <div class="panel-head"><h2>交易</h2></div>
            <div class="panel-body tradegrid">
              <OrderPanel :stock="s" :pick="pick" @placed="afterOrder" />
              <OrderBook :bids="s.book.bids" :asks="s.book.asks" :prev-close="q.prev_close" @pick="onPick" />
            </div>
          </section>

          <section class="panel" v-if="s.position || s.my_orders?.length">
            <div class="panel-head"><h2>我的</h2></div>
            <div class="panel-body mine">
              <dl v-if="s.position" class="facts small">
                <div><dt>持有</dt><dd class="num">{{ s.position.qty }} 股（可卖 {{ s.position.available }}）</dd></div>
                <div><dt>成本价</dt><dd class="num">{{ money(s.position.avg_cost) }}</dd></div>
                <div><dt>市值</dt><dd class="num">{{ money(s.position.value) }}</dd></div>
                <div><dt>浮动盈亏</dt><dd class="num" :class="dir(s.position.pnl)">{{ signed(s.position.pnl) }}（{{ pct(s.position.pnl_pct) }}）</dd></div>
                <div><dt>累计分红</dt><dd class="num up">{{ money(s.position.dividends) }}</dd></div>
                <div><dt>含分红总盈亏</dt><dd class="num" :class="dir(s.position.pnl + s.position.dividends)">{{ signed(s.position.pnl + s.position.dividends) }}</dd></div>
              </dl>
              <div v-if="s.my_orders?.length">
                <span class="label">本股委托</span>
                <ul class="orders">
                  <li v-for="o in s.my_orders" :key="o.id">
                    <span :class="o.side === 'buy' ? 'up' : 'down'">{{ o.side === 'buy' ? '买' : '卖' }}</span>
                    <span class="num">{{ money(o.price) }} × {{ o.qty }}</span>
                    <span class="num muted">已成 {{ o.filled }}</span>
                    <button type="button" class="linkbtn" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button>
                  </li>
                </ul>
              </div>
              <p v-if="actionMsg" class="notice error">{{ actionMsg }}</p>
            </div>
          </section>

          <section class="panel">
            <div class="panel-head"><h2>最近成交</h2></div>
            <table class="grid" v-if="s.trades.length">
              <thead><tr><th>时间</th><th>价格</th><th>股数</th></tr></thead>
              <tbody>
                <tr v-for="(t, i) in s.trades.slice(0, 12)" :key="i">
                  <td class="num muted">{{ time(t.ts) }}</td>
                  <td class="num" :class="dir(t.price - q.prev_close)">{{ money(t.price) }}</td>
                  <td class="num">{{ t.qty }}</td>
                </tr>
              </tbody>
            </table>
            <p v-else class="empty">还没有成交</p>
          </section>
        </div>
      </div>
    </template>

    <template v-else-if="!error">
      <div class="panel" style="padding: 16px"><div class="skeleton" style="height: 90px"></div></div>
      <div class="panel" style="padding: 16px"><div class="skeleton" style="height: 260px"></div></div>
    </template>
  </main>
</template>

<style scoped>
.back { color: var(--ink-2); }
.quote { padding: 12px; display: flex; flex-direction: column; gap: 10px; }
.qtitle h1 { font-size: 18px; font-weight: 700; overflow-wrap: anywhere; }
.ja { margin-top: 2px; overflow-wrap: anywhere; }
.meta { display: flex; flex-wrap: wrap; gap: 4px 8px; align-items: center; margin-top: 4px; }
.qprice { display: flex; align-items: baseline; gap: 12px; flex-wrap: wrap; }
.price { font-size: 30px; font-weight: 700; line-height: 1; }
.banner { display: flex; flex-direction: column; gap: 2px; font-size: 13px; padding: 8px 10px; border-radius: var(--radius); border-left: 3px solid; }
.banner.vcm { background: var(--warn-bg); color: var(--warn); border-color: var(--warn); }
.banner.auction { background: var(--surface-2); color: var(--ink-2); border-color: var(--accent); }
.banner b { color: inherit; }
.qstats { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px 10px; margin: 0; }
.qstats div { display: flex; flex-direction: column; }
.qstats dt { color: var(--muted); }
.qstats dd { margin: 0; }

.cols { display: flex; flex-direction: column; gap: 12px; }
.col { display: flex; flex-direction: column; gap: 12px; min-width: 0; }
.side { order: -1; }
@media (min-width: 900px) {
  .cols { display: grid; grid-template-columns: minmax(0, 1fr) 380px; align-items: start; }
  .side { order: 0; position: sticky; top: 60px; }
  .qstats { grid-template-columns: repeat(8, minmax(0, 1fr)); }
}

.tradegrid { display: grid; grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr); gap: 12px; }
@media (max-width: 380px) { .tradegrid { grid-template-columns: 1fr; } }

.fund { display: flex; flex-direction: column; gap: 14px; }
.facts { margin: 0; display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px 12px; }
.facts div { display: flex; flex-direction: column; min-width: 0; }
.facts dt { color: var(--muted); }
.facts dd { margin: 0; overflow-wrap: anywhere; }
.holders, .aud, .divs { display: flex; flex-direction: column; gap: 6px; }
.hbar { display: flex; height: 8px; border-radius: 4px; overflow: hidden; background: var(--surface-2); }
.hbar span { display: block; height: 100%; }
.hlegend { display: flex; flex-wrap: wrap; gap: 2px 12px; color: var(--ink-2); }
.hlegend i { display: inline-block; width: 8px; height: 8px; border-radius: 2px; margin-right: 4px; }
.audgrid { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 8px; }
.audgrid > div { display: flex; flex-direction: column; gap: 1px; padding: 8px; background: var(--surface-2); border-radius: 4px; }
.mk { font-weight: 600; font-size: 12px; }
.divlist { display: flex; flex-wrap: wrap; gap: 4px 14px; color: var(--ink-2); }
.divlist b { color: var(--up); font-weight: 600; }
.news { list-style: none; margin: 0; padding: 0; }
.news li { display: flex; flex-direction: column; gap: 2px; padding: 10px 12px; border-bottom: 1px solid var(--rule); }
.news li:last-child { border-bottom: none; }
.mine { display: flex; flex-direction: column; gap: 10px; }
.orders { list-style: none; margin: 4px 0 0; padding: 0; font-size: 13px; }
.orders li { display: grid; grid-template-columns: 1.5em 1fr auto auto; gap: 8px; align-items: center; padding: 6px 0; border-bottom: 1px solid var(--rule); }
.orders li:last-child { border-bottom: none; }
</style>
