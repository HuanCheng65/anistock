// 指数基金 (docs/design/funds.md): the funds, one fund's net values and holdings, and the player's units and
// requests. Net values are 星辉 a unit; units come in hundredths (120355 is 1,203.55 份).
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, call, type FundDetail, type FundView, type MyFunds } from '../api'

export const useFunds = defineStore('funds', () => {
  /** null: not loaded; [] : funds not open */
  const list = ref<FundView[] | null>(null)
  const detail = ref<Record<string, FundDetail>>({})
  const mine = ref<MyFunds | null>(null)

  async function refreshList() {
    try {
      list.value = (await call(api.GET('/api/v1/funds'))).funds
    } catch (e) {
      // funds_closed: the funds are not open yet
      if ((e as { code?: string })?.code === 'funds_closed') list.value = []
      else throw e
    }
  }
  async function refresh(key: string) {
    detail.value[key] = await call(api.GET('/api/v1/funds/{key}', { params: { path: { key } } }))
  }
  async function refreshMine() {
    try {
      mine.value = await call(api.GET('/api/v1/me/funds'))
    } catch (e) {
      if ((e as { code?: string })?.code === 'funds_closed') mine.value = null
      else throw e
    }
  }
  async function subscribe(fund: string, amount: number) {
    const r = await call(api.POST('/api/v1/me/fund-requests', { body: { fund, kind: 'subscribe', amount } }))
    mine.value = r.funds
    return r.request
  }
  async function redeem(fund: string, units: number) {
    const r = await call(api.POST('/api/v1/me/fund-requests', { body: { fund, kind: 'redeem', units } }))
    mine.value = r.funds
    return r.request
  }
  async function cancel(id: number) {
    mine.value = await call(api.DELETE('/api/v1/me/fund-requests/{id}', { params: { path: { id } } }))
  }
  return { list, detail, mine, refreshList, refresh, refreshMine, subscribe, redeem, cancel }
})
