// One stock: the detail object, its book, recent trades, news, candles and intraday points, kept per id.
// While a stock page is open the `stock:<id>` topic keeps quote, book, VCM and trades current.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { onLive } from '../live'
import {
  api, ApiError, call,
  type AuctionIndication, type BookLevel, type Candle, type ExDay, type IntradayPoint, type NewsItem, type PublicTrade, type RightsOffer,
  type StockDetail, type WorksResponse,
} from '../api'
import { tradingDayOf } from '../format'

/** The stock page's 公告: company notices, airing news, dividends and flashes; not 冷静期 or rate news. */
export const NOTICE_KINDS = [
  'flash', 'premiere', 'finale', 'delay', 'hiatus', 'dividend', 'sequel', 'split', 'rights', 'buyback',
  'delisting', 'warning', 'ipo', 'allotment',
].join(',')

export interface Book {
  bids: BookLevel[]
  asks: BookLevel[]
  auction: AuctionIndication | null
}

export const useStock = defineStore('stock', () => {
  const detail = ref<Record<number, StockDetail>>({})
  const book = ref<Record<number, Book>>({})
  const trades = ref<Record<number, PublicTrade[]>>({})
  const news = ref<Record<number, NewsItem[]>>({})
  /** the cursor of the next older page of `news` (null: no more) */
  const newsMore = ref<Record<number, string | null>>({})
  const works = ref<Record<number, WorksResponse>>({})
  /** the AniEx heat index by day, summed over the company's works (docs/v3-futures.md §3); `backfill`: a day
   *  before the live index, rebuilt from the platforms' archives (server/tools/import_heat.py --backfill) */
  const heat = ref<Record<number, { day: string; index: number; backfill: boolean }[]>>({})
  const candles = ref<Record<number, Candle[]>>({})
  const intraday = ref<Record<number, IntradayPoint[]>>({})
  // corporate actions (docs/v2-corporate.md): split / ex-rights days of the K-line, the open rights issue
  const exDays = ref<Record<number, ExDay[]>>({})
  const rights = ref<Record<number, RightsOffer | null>>({})
  /** K-lines forward-adjusted (前复权, default) or as traded */
  const adjust = ref<'forward' | 'none'>('forward')
  const error = ref<Record<number, string>>({})
  const candleAt: Record<number, number> = {}
  const newsAt: Record<number, number> = {}
  const worksAt: Record<number, number> = {}
  const heatAt: Record<number, number> = {}

  async function refresh(id: number, force = false) {
    try {
      const [d, b, t] = await Promise.all([
        call(api.GET('/api/v1/stocks/{id}', { params: { path: { id } } })),
        call(api.GET('/api/v1/stocks/{id}/book', { params: { path: { id } } })),
        call(api.GET('/api/v1/stocks/{id}/trades', { params: { path: { id }, query: { limit: 30 } } })),
      ])
      detail.value[id] = d
      book.value[id] = { bids: b.bids, asks: b.asks, auction: b.auction }
      trades.value[id] = t.items
      error.value[id] = ''
    } catch (e) {
      error.value[id] = e instanceof ApiError ? (e.status === 404 ? '股票不存在' : e.message) : '无法加载'
      throw e
    }
    const now = Date.now()
    const jobs: Promise<unknown>[] = []
    if (force || !candles.value[id] || now - (candleAt[id] ?? 0) > 30000) {
      candleAt[id] = now
      jobs.push(
        call(api.GET('/api/v1/stocks/{id}/candles', { params: { path: { id }, query: { adjust: adjust.value } } })).then((r) => {
          candles.value[id] = r.items
          exDays.value[id] = r.ex_days
        }),
      )
      jobs.push(
        call(api.GET('/api/v1/stocks/{id}/rights', { params: { path: { id } } }))
          .then((r) => (rights.value[id] = r.action ? r : null))
          .catch(() => {}),
      )
    }
    if (force || !news.value[id] || now - (newsAt[id] ?? 0) > 60000) {
      newsAt[id] = now
      jobs.push(
        call(api.GET('/api/v1/news', { params: { query: { stock_id: id, kind: NOTICE_KINDS, limit: 30 } } }))
          .then((r) => {
            // keep older pages already loaded: merge the fresh first page in front
            const fresh = new Set(r.items.map((n) => n.id))
            const old = (news.value[id] ?? []).filter((n) => !fresh.has(n.id) && n.id < (r.items.at(-1)?.id ?? Infinity))
            news.value[id] = [...r.items, ...old]
            if (!old.length) newsMore.value[id] = r.has_more ? r.next_cursor : null
          })
          .catch(() => {}),
      )
    }
    await Promise.all(jobs)
  }

  async function moreNews(id: number) {
    const before = newsMore.value[id]
    if (!before) return
    const r = await call(api.GET('/api/v1/news', { params: { query: { stock_id: id, kind: NOTICE_KINDS, limit: 30, before } } }))
    const have = new Set((news.value[id] ?? []).map((n) => n.id))
    news.value[id] = [...(news.value[id] ?? []), ...r.items.filter((n) => !have.has(n.id))]
    newsMore.value[id] = r.has_more ? r.next_cursor : null
  }

  /** works, schedule and sequel (changes at most daily: refreshed every 5 minutes while shown) */
  async function refreshWorks(id: number, force = false) {
    const now = Date.now()
    if (!force && works.value[id] && now - (worksAt[id] ?? 0) < 300_000) return
    worksAt[id] = now
    works.value[id] = await call(api.GET('/api/v1/stocks/{id}/works', { params: { path: { id } } }))
  }

  async function refreshIntraday(id: number) {
    intraday.value[id] = (await call(api.GET('/api/v1/stocks/{id}/intraday', { params: { path: { id } } }))).items
  }

  // pushes for the open stock page (topic stock:<id>), conflated to 100 ms on the server
  onLive('stock', (m) => {
    const d = detail.value[m.id]
    if (!d) return
    const q = d.quote
    q.price = m.price
    q.prev_close = m.prev_close
    q.change = m.prev_close ? m.price / m.prev_close - 1 : 0
    q.open = m.open
    q.high = m.high
    q.low = m.low
    q.volume = m.volume
    q.turnover = m.turnover
    q.open_reference = m.open_reference
    q.market_cap = m.price * d.shares
    d.vcm = m.vcm
    const lv = (l: number[][]) => l.map(([price, quantity, orders]) => ({ price, quantity, orders }))
    book.value[m.id] = { bids: lv(m.bids), asks: lv(m.asks), auction: m.auction }
    const pushed = (m.trades as unknown as [number, number, number, string][]).map(([id, price, quantity, at]) => ({
      id, price, quantity, at, trading_day: tradingDayOf(at),
    }))
    if (m.full) {
      // the full snapshot carries the last 30 trades; keep anything newer we already have
      const known = trades.value[m.id] ?? []
      const ids = new Set(pushed.map((t) => t.id))
      trades.value[m.id] = [...pushed, ...known.filter((t) => !ids.has(t.id))].sort((a, b) => b.id - a.id).slice(0, 60)
      return
    }
    if (!pushed.length) return
    const known = trades.value[m.id] ?? []
    const have = new Set(known.map((t) => t.id))
    const fresh = pushed.filter((t) => !have.has(t.id))
    trades.value[m.id] = [...fresh, ...known].slice(0, 60)
    const pts = intraday.value[m.id]
    if (pts) {
      // trades arrive newest first; fold them into the minute points
      for (const t of [...fresh].reverse()) {
        const minute = t.at.slice(0, 16)
        const last = pts[pts.length - 1]
        if (last && last.minute.slice(0, 16) === minute) {
          last.price = t.price
          last.volume += t.quantity
        } else if (!last || last.minute.slice(0, 16) < minute) pts.push({ minute: minute + ':00' + t.at.slice(19), price: t.price, volume: t.quantity })
      }
    }
  })

  /** the heat index (published once a day after the close: refreshed every 5 minutes while shown) */
  async function refreshHeat(id: number, force = false) {
    const now = Date.now()
    if (!force && heat.value[id] && now - (heatAt[id] ?? 0) < 300_000) return
    heatAt[id] = now
    const r = await call(api.GET('/api/v1/stocks/{id}/heat', { params: { path: { id } } }))
    const by = new Map<string, { index: number; backfill: boolean }>()
    for (const w of r.works) {
      for (const d of w.days) {
        const x = by.get(d.day) ?? { index: 0, backfill: false }
        by.set(d.day, { index: x.index + d.index, backfill: x.backfill || d.version.endsWith('-backfill') })
      }
    }
    heat.value[id] = [...by].sort((a, b) => (a[0] < b[0] ? -1 : 1)).map(([day, x]) => ({ day, ...x }))
  }

  return {
    detail, book, trades, news, newsMore, works, heat, candles, intraday, exDays, rights, adjust, error,
    refresh, refreshIntraday, moreNews, refreshWorks, refreshHeat,
  }
})
