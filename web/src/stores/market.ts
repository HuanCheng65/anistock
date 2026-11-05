// Market-wide data: quotes for every listed stock, indices, tonight's airings, headlines.
// Each domain has a refresh method; views poll them now, a push channel can call the same setters later.
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, type Airing, type Indices, type MarketStock, type NewsItem } from '../api'

export const useMarket = defineStore('market', () => {
  const stocks = ref<MarketStock[] | null>(null)
  const indices = ref<Indices | null>(null)
  const tonight = ref<Airing[] | null>(null)
  const news = ref<NewsItem[] | null>(null)
  const error = ref('')

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

  return { stocks, indices, tonight, news, error, byId, refreshQuotes, refreshIndices, refreshTonight, refreshNews }
})
