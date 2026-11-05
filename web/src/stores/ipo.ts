// New listings: open subscriptions with allotment results, and recent listings.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, type Ipo, type Listings } from '../api'

export const useIpo = defineStore('ipo', () => {
  const ipos = ref<Ipo[] | null>(null)
  const listings = ref<Listings | null>(null)

  async function refresh() {
    const [a, b] = await Promise.all([
      api.get<{ ipos: Ipo[] }>('/api/ipos'),
      api.get<Listings>('/api/v2/listings').catch(() => ({ recent: [], upcoming: [] })),
    ])
    ipos.value = a.ipos
    listings.value = b
  }
  async function subscribe(id: number, q: number) {
    if (q === 0) await api.del(`/api/ipos/${id}/subscribe`)
    else await api.post(`/api/ipos/${id}/subscribe`, { qty: q })
    await refresh()
  }
  return { ipos, listings, refresh, subscribe }
})
