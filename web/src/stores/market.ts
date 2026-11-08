// Market-wide data: quotes for every listed stock (ratios included), indices, the airings of this trading
// day, headlines, macro rows. Quotes and news also arrive over the WebSocket.
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { onLive } from '../live'
import { api, call, type Airing, type IndicesResponse, type MacroRow, type NewsItem, type Quote } from '../api'

export const useMarket = defineStore('market', () => {
  const stocks = ref<Quote[] | null>(null)
  const indices = ref<IndicesResponse | null>(null)
  const tonight = ref<Airing[] | null>(null)
  const news = ref<NewsItem[] | null>(null)
  const macro = ref<MacroRow[] | null>(null)
  const error = ref('')

  const byId = computed(() => new Map((stocks.value ?? []).map((s) => [s.stock.id, s])))

  async function refreshQuotes() {
    try {
      merge((await call(api.GET('/api/v1/market/quotes'))).items)
      error.value = ''
    } catch (e: any) {
      error.value = e?.message ?? '无法加载行情'
      throw e
    }
  }
  /** Fold a fresh quotes list into the rows already shown: an unchanged field (sparklines and ratios included)
   *  keeps its value, so a poll re-renders only what moved. A new or removed stock replaces the list. */
  function merge(items: Quote[]) {
    const cur = stocks.value
    if (!cur || cur.length !== items.length || items.some((q, i) => q.stock.id !== cur[i].stock.id)) {
      stocks.value = items
      return
    }
    items.forEach((q, i) => {
      const x = cur[i] as unknown as Record<string, unknown>
      for (const [k, v] of Object.entries(q)) {
        const old = x[k]
        if (old === v) continue
        if (v !== null && typeof v === 'object' && JSON.stringify(old) === JSON.stringify(v)) continue
        x[k] = v
      }
    })
  }

  async function refreshIndices() {
    try {
      indices.value = await call(api.GET('/api/v1/market/indices'))
    } catch {
      /* optional block: the page renders without it */
    }
  }
  async function refreshTonight() {
    try {
      tonight.value = (await call(api.GET('/api/v1/market/airings'))).items
    } catch (e) {
      tonight.value = tonight.value ?? []
      throw e // usePoll retries soon
    }
  }
  async function refreshNews(limit = 50) {
    news.value = (await call(api.GET('/api/v1/news', { params: { query: { limit } } }))).items
  }
  async function refreshMacro() {
    try {
      macro.value = (await call(api.GET('/api/v1/market/macro'))).items
    } catch (e) {
      // the block shows – until usePoll's retry
      throw e
    }
  }

  // pushes: quotes conflated to ~1 s (only changed rows unless `full`), news as posted
  onLive('quotes', (m) => {
    const map = byId.value
    const cols = m.cols ?? ['id', 'price', 'prev_close', 'volume', 'turnover', 'vcm_active']
    const at = (k: string) => cols.indexOf(k)
    const [iId, iP, iPc, iV, iT, iC] = ['id', 'price', 'prev_close', 'volume', 'turnover', 'vcm_active'].map(at)
    for (const row of m.rows as unknown[][]) {
      const x = map.get(row[iId] as number)
      if (!x) continue
      const price = row[iP] as number
      const prev = row[iPc] as number
      if (price !== x.price) {
        // keep the day's range current without another request
        x.high = x.high == null ? price : Math.max(x.high, price)
        x.low = x.low == null ? price : Math.min(x.low, price)
        if (x.open == null) x.open = price
      }
      x.price = price
      x.prev_close = prev
      x.change = prev ? price / prev - 1 : 0
      x.volume = row[iV] as number
      x.turnover = row[iT] as number
      x.vcm_active = row[iC] as boolean
    }
  })
  // the server could not replay what the socket missed (too far behind): read the headlines again
  onLive('reset', (m) => {
    if (m.topic === 'news' && news.value) refreshNews(Math.max(news.value.length, 40)).catch(() => {})
  })
  /** ids of items that arrived over the push channel (the views slide them in) */
  const fresh = ref(new Set<number>())
  onLive('news', (m) => {
    if (!news.value || news.value.some((n) => n.id === m.item.id)) return
    news.value = [m.item, ...news.value]
    fresh.value.add(m.item.id)
    window.setTimeout(() => fresh.value.delete(m.item.id), 3000)
  })

  return { fresh, macro, refreshMacro, stocks, indices, tonight, news, error, byId, refreshQuotes, refreshIndices, refreshTonight, refreshNews }
})
