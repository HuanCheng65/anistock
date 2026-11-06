// One stock: the detail object, its book, recent trades, news, candles and intraday points, kept per id.
// While a stock page is open the `stock:<id>` topic keeps quote, book, VCM and trades current.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { onLive } from '../live'
import {
  api, ApiError, call,
  type AuctionIndication, type BookLevel, type Candle, type ExDay, type IntradayPoint, type NewsItem, type PublicTrade, type RightsOffer,
  type StockDetail,
} from '../api'
import { tradingDayOf } from '../format'

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
        call(api.GET('/api/v1/news', { params: { query: { stock_id: id, limit: 20 } } }))
          .then((r) => (news.value[id] = r.items))
          .catch(() => {}),
      )
    }
    await Promise.all(jobs)
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

  return { detail, book, trades, news, candles, intraday, exDays, rights, adjust, error, refresh, refreshIntraday }
})
