// Market-wide data: quotes for every listed stock, indices, tonight's airings, headlines.
// Each domain has a refresh method; views poll them now, a push channel can call the same setters later.
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { onLive } from '../live'
import { api, type Airing, type Indices, type MacroDay, type MacroSeries, type MarketStock, type NewsItem, type Ratios } from '../api'

export const useMarket = defineStore('market', () => {
  const stocks = ref<MarketStock[] | null>(null)
  const indices = ref<Indices | null>(null)
  const tonight = ref<Airing[] | null>(null)
  const news = ref<NewsItem[] | null>(null)
  const error = ref('')
  const ratios = ref<Ratios | null>(null)
  const macro = ref<MacroSeries | null>(null)
  const macroDays = ref<MacroDay[] | null>(null)
  const ratioById = computed(() => new Map((ratios.value?.rows ?? []).map((r) => [r.id, r])))

  const byId = computed(() => new Map((stocks.value ?? []).map((s) => [s.id, s])))

  async function refreshQuotes() {
    try {
      stocks.value = (await api.get<{ stocks: MarketStock[] }>('/api/market')).stocks
      error.value = ''
    } catch (e: any) {
      error.value = e?.message ?? '无法加载行情'
      throw e
    }
  }
  async function refreshIndices() {
    try {
      indices.value = await api.get<Indices>('/api/v2/indices')
    } catch {
      /* optional block: the page renders without it */
    }
  }
  async function refreshTonight() {
    try {
      tonight.value = (await api.get<{ airings: Airing[] }>('/api/v2/tonight')).airings
    } catch {
      tonight.value = tonight.value ?? []
    }
  }
  async function refreshNews(limit = 150) {
    news.value = (await api.get<{ news: NewsItem[] }>(`/api/news?limit=${limit}`)).news
  }

  async function refreshRatios() {
    try {
      ratios.value = await api.get<Ratios>('/api/v2/ratios')
    } catch {
      /* the table shows – */
    }
  }
  async function refreshMacro() {
    const [a, b] = await Promise.all([
      api.get<MacroSeries>('/api/v2/macro').catch(() => null),
      api.get<{ days: MacroDay[] }>('/api/macro').then((r) => r.days).catch(() => null),
    ])
    macro.value = a
    macroDays.value = b
  }

  // pushes (live.ts): quotes conflated to ~1 s, news as posted
  onLive('quotes', (m: { full: boolean; s: [number, number, number, number, number, boolean][] }) => {
    const map = byId.value
    for (const [id, price, prev, volume, turnover, cooling] of m.s) {
      const x = map.get(id)
      if (!x) continue
      x.price = price
      x.prev_close = prev
      x.change = prev ? (price - prev) / prev : 0
      x.volume = volume
      x.turnover = turnover
      x.cooling = cooling
    }
  })
  /** ids of items that arrived over the push channel (the views slide them in) */
  const fresh = ref(new Set<number>())
  onLive('news', (m: { item: NewsItem }) => {
    if (!news.value || news.value.some((n) => n.id === m.item.id)) return
    news.value = [m.item, ...news.value]
    fresh.value.add(m.item.id)
    window.setTimeout(() => fresh.value.delete(m.item.id), 3000)
  })

  return { fresh, ratios, ratioById, macro, macroDays, refreshRatios, refreshMacro, stocks, indices, tonight, news, error, byId, refreshQuotes, refreshIndices, refreshTonight, refreshNews }
})
