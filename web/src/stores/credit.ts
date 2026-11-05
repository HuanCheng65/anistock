// 融资融券: the player's credit account, opening it, direct repayment, and each stock's margin terms
// (docs/v2-margin.md). The `credit` push on the `me` topic carries status changes (追保, 强制平仓, 穿仓).
import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api, call, type CreditAccount, type StockMargin } from '../api'
import { onLive } from '../live'
import { useSession } from './session'

export const MODE_LABEL: Record<string, string> = {
  margin_buy: '融资买入',
  short_sell: '融券卖出',
  sell_repay: '卖券还款',
  buy_return: '买券还券',
}

export const useCredit = defineStore('credit', () => {
  const account = ref<CreditAccount | null>(null)
  const margins = ref<Record<number, StockMargin>>({})

  /** credit is open and usable for new orders (not after a default) */
  const usable = computed(() => {
    const s = useSession().me?.credit_status
    return s != null && s !== 'defaulted'
  })

  async function refresh() {
    account.value = (await call(api.GET('/api/v1/me/credit'))).credit
  }
  async function open() {
    account.value = (await call(api.POST('/api/v1/me/credit'))).credit
    await useSession().loadMe()
  }
  async function repay(amount: number | null) {
    const r = await call(api.POST('/api/v1/me/credit/repay', { body: { amount } }))
    account.value = r.credit
    await useSession().loadMe()
    return r.paid
  }
  async function loadMargin(id: number) {
    margins.value = { ...margins.value, [id]: await call(api.GET('/api/v1/stocks/{id}/margin', { params: { path: { id } } })) }
  }
  function clear() {
    account.value = null
  }

  onLive('credit', (m) => {
    const me = useSession().me
    if (me) me.credit_status = m.status
    if (account.value) refresh().catch(() => {})
  })

  return { account, margins, usable, refresh, open, repay, loadMargin, clear }
})
