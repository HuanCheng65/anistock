// The signed-in player's positions, open orders, trades and cash ledger.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, type LedgerEntry, type Order, type Position, type Trade } from '../api'
import { useSession } from './session'

export const useAccount = defineStore('account', () => {
  const positions = ref<Position[] | null>(null)
  const orders = ref<Order[] | null>(null)
  const trades = ref<Trade[] | null>(null)
  const ledger = ref<LedgerEntry[] | null>(null)

  async function refreshPositions() {
    positions.value = (await api.get<{ positions: Position[] }>('/api/portfolio')).positions
  }
  async function refreshOrders() {
    orders.value = (await api.get<{ orders: Order[] }>('/api/orders?status=open')).orders
  }
  async function refreshTrades() {
    trades.value = (await api.get<{ trades: Trade[] }>('/api/trades')).trades
  }
  async function refreshLedger() {
    ledger.value = (await api.get<{ entries: LedgerEntry[] }>('/api/ledger')).entries
  }
  async function cancel(id: number) {
    await api.del(`/api/orders/${id}`)
    await Promise.all([refreshOrders(), useSession().loadMe()])
  }
  function clear() {
    positions.value = orders.value = trades.value = ledger.value = null
  }

  return { positions, orders, trades, ledger, refreshPositions, refreshOrders, refreshTrades, refreshLedger, cancel, clear }
})
