// New listings: subscriptions (upcoming, open), allotted ones waiting to list, and recent listings.
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, call, type Ipo } from '../api'

export const useIpo = defineStore('ipo', () => {
  const ipos = ref<Ipo[] | null>(null)
  const open = computed(() => (ipos.value ?? []).filter((i) => i.status === 'open'))
  const upcoming = computed(() => (ipos.value ?? []).filter((i) => i.status === 'upcoming'))
  const allotted = computed(() => (ipos.value ?? []).filter((i) => i.status === 'allotted'))
  const listed = computed(() => (ipos.value ?? []).filter((i) => i.status === 'listed'))

  async function refresh() {
    ipos.value = (await call(api.GET('/api/v1/ipos', { params: { query: { status: 'upcoming,open,allotted,listed' } } }))).items
  }
  /** Set my subscription to `q` shares; 0 withdraws it. */
  async function subscribe(id: number, q: number) {
    if (q === 0) await call(api.DELETE('/api/v1/ipos/{id}/subscription', { params: { path: { id } } }))
    else await call(api.PUT('/api/v1/ipos/{id}/subscription', { params: { path: { id } }, body: { quantity: q } }))
    await refresh()
  }
  return { ipos, open, upcoming, allotted, listed, refresh, subscribe }
})
