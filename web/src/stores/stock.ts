// One stock's detail, candles, profile and intraday points, kept per id.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { onLive } from '../live'
import { api, ApiError, type Candle, type IntradayPoint, type Profile, type StockDetail } from '../api'

interface LiveStock {
  id: number
  init: boolean
  price: number
  prev_close: number
  open: number | null
  high: number | null
  low: number | null
  volume: number
  turnover: number
  bids: [number, number, number][]
  asks: [number, number, number][]
  vcm: { active: boolean; until: string | null; low: number | null; high: number | null; count: number } | null
  trades: [number, number, number, string][]
}

export const useStock = defineStore('stock', () => {
  const detail = ref<Record<number, StockDetail>>({})
  const candles = ref<Record<number, Candle[]>>({})
  const profile = ref<Record<number, Profile>>({})
  const intraday = ref<Record<number, IntradayPoint[]>>({})
  const error = ref<Record<number, string>>({})
  const candleAt: Record<number, number> = {}
  const profileAt: Record<number, number> = {}

  async function refresh(id: number, force = false) {
    try {
      detail.value[id] = await api.get<StockDetail>(`/api/stocks/${id}`)
      error.value[id] = ''
    } catch (e) {
      error.value[id] = e instanceof ApiError ? (e.status === 404 || e.status === 422 ? '股票不存在' : e.message) : '无法加载'
      throw e
    }
    const now = Date.now()
    const jobs: Promise<unknown>[] = []
    if (force || !candles.value[id] || now - (candleAt[id] ?? 0) > 30000) {
      candleAt[id] = now
      jobs.push(api.get<{ candles: Candle[] }>(`/api/stocks/${id}/candles`).then((r) => (candles.value[id] = r.candles)))
    }
    if (force || !profile.value[id] || now - (profileAt[id] ?? 0) > 60000) {
      profileAt[id] = now
      jobs.push(api.get<Profile>(`/api/v2/stocks/${id}/profile`).then((r) => (profile.value[id] = r)).catch(() => {}))
    }
    await Promise.all(jobs)
  }

  async function refreshIntraday(id: number) {
    intraday.value[id] = (await api.get<{ points: IntradayPoint[] }>(`/api/stocks/${id}/intraday`)).points
  }

  // pushes for the open stock page (topic stock:<id>), conflated to 100 ms on the server
  onLive('stock', (m: LiveStock) => {
    const d = detail.value[m.id]
    if (!d) return
    const q = d.quote
    q.price = m.price
    q.prev_close = m.prev_close
    q.open = m.open
    q.high = m.high
    q.low = m.low
    q.volume = m.volume
    q.turnover = m.turnover
    const lv = (l: [number, number, number][]) => l.map(([price, qty, orders]) => ({ price, qty, orders }))
    d.book = { bids: lv(m.bids), asks: lv(m.asks) }
    d.vcm = m.vcm ?? (d.vcm ? { ...d.vcm, active: false } : undefined)
    if (m.trades.length) {
      d.trades = [...m.trades.map(([id, price, qty, ts]) => ({ id, price, qty, ts })), ...d.trades].slice(0, 60)
      const pts = intraday.value[m.id]
      if (pts) {
        // trades arrive newest first; fold them into the minute points
        for (const [, price, qty, ts] of [...m.trades].reverse()) {
          const minute = ts.slice(0, 16)
          const last = pts[pts.length - 1]
          if (last && last.minute.slice(0, 16) === minute) {
            last.price = price
            last.volume += qty
          } else if (!last || last.minute.slice(0, 16) < minute) pts.push({ minute, price, volume: qty })
        }
      }
    }
  })

  return { detail, candles, profile, intraday, error, refresh, refreshIntraday }
})
