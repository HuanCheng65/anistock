// One stock's detail, candles, profile and intraday points, kept per id.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, ApiError, type Candle, type IntradayPoint, type Profile, type StockDetail } from '../api'

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

  return { detail, candles, profile, intraday, error, refresh, refreshIntraday }
})
