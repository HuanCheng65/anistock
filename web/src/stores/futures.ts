// 热度期货 (docs/design/futures.md §4–5): the contracts, one contract's market, and the player's futures. Prices are
// in hundredths of an index point; a contract's tick_value is what one hundredth of one lot is worth.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, call, type FuturesContract, type FuturesDetail, type MyFutures } from '../api'
import { onLive } from '../live'

export const useFutures = defineStore('futures', () => {
  /** the contracts trading or waiting for their index (null: not loaded) */
  const list = ref<FuturesContract[] | null>(null)
  const detail = ref<Record<number, FuturesDetail>>({})
  const mine = ref<MyFutures | null>(null)
  const error = ref<Record<number, string>>({})

  async function refreshList() {
    list.value = (await call(api.GET('/api/v1/futures'))).contracts
  }
  async function refresh(id: number) {
    detail.value[id] = await call(api.GET('/api/v1/futures/{id}', { params: { path: { id } } }))
  }
  async function refreshMine() {
    mine.value = await call(api.GET('/api/v1/me/futures'))
  }
  async function place(body: { contract_id: number; side: 'buy' | 'sell'; quantity: number; price: number }) {
    const r = await call(api.POST('/api/v1/me/futures/orders', { body: { ...body, type: 'limit' } }))
    // the answer is shown at once; the book and the player's futures catch up behind it
    Promise.all([refresh(body.contract_id), refreshMine()]).catch(() => {})
    return r
  }
  async function cancel(orderId: number) {
    mine.value = await call(api.DELETE('/api/v1/me/futures/orders/{id}', { params: { path: { id: orderId } } }))
  }
  /** the contract trading on a stock now (one per work at a time; the latest period when two overlap) */
  function forStock(stockId: number): FuturesContract | null {
    const xs = (list.value ?? []).filter((c) => c.stock.id === stockId && c.status === 'listed')
    return xs.sort((a, b) => b.period - a.period)[0] ?? null
  }

  // push (docs/api-v1.md §5): `futures:{id}` snapshots replace the contract and its book; `futures_mine` on
  // `me` says the player's orders or positions changed, read again at most once a second
  onLive('futures', (m) => {
    const d = detail.value[m.contract.id]
    if (d) detail.value[m.contract.id] = { ...d, contract: m.contract, bids: m.bids, asks: m.asks }
    const i = list.value?.findIndex((c) => c.id === m.contract.id) ?? -1
    if (list.value && i >= 0) list.value[i] = m.contract
  })
  let mineTimer: number | null = null
  onLive('futures_mine', () => {
    if (mineTimer != null) return
    mineTimer = window.setTimeout(() => {
      mineTimer = null
      refreshMine().catch(() => {})
    }, 1000)
  })

  return { list, detail, mine, error, refreshList, refresh, refreshMine, place, cancel, forStock }
})
