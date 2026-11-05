// The signed-in player's positions, open orders, trades and cash ledger.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, type LedgerEntry, type Order, type Position, type Trade } from '../api'
import { money, qty } from '../format'
import { onLive } from '../live'
import { useSession } from './session'
import { useToast } from './toast'

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

  // ---- pushes on the `me` topic: own orders, fills and cash. Totals (市值, 盈亏) are the server's, so a
  // push only triggers a refetch of what is already loaded, at most every 400 ms.
  let resync: number | undefined
  function soon() {
    window.clearTimeout(resync)
    resync = window.setTimeout(() => {
      const session = useSession()
      session.loadMe().catch(() => {})
      if (orders.value) refreshOrders().catch(() => {})
      if (positions.value) refreshPositions().catch(() => {})
      if (trades.value) refreshTrades().catch(() => {})
    }, 400)
  }
  // fills of one order within a moment are summed into one toast
  const fills = new Map<number, { qty: number; notional: number; at: number }>()
  onLive('fill', (m: { side: 'buy' | 'sell'; title: string | null; price: number; qty: number; order_id: number | null }) => {
    const key = m.order_id ?? -1
    const now = Date.now()
    const prev = fills.get(key)
    const f = prev && now - prev.at < 3000 ? prev : { qty: 0, notional: 0, at: now }
    f.qty += m.qty
    f.notional += m.qty * m.price
    f.at = now
    fills.set(key, f)
    const avg = f.notional / f.qty
    useToast().show({
      key: 'fill' + key,
      text: `${m.side === 'buy' ? '买入' : '卖出'}成交 ${qty(f.qty)} 股 · ${money(avg)}`,
      sub: m.title ?? undefined,
      tone: m.side,
    })
    soon()
  })
  onLive('order', (m: { order: Order; open: boolean }) => {
    if (orders.value) {
      const rest = orders.value.filter((o) => o.id !== m.order.id)
      orders.value = m.open ? [m.order, ...rest].sort((a, b) => b.id - a.id) : rest
    }
    soon()
  })
  onLive('cash', (m: { cash: number; frozen: number; available: number }) => {
    const me = useSession().me
    if (me) {
      me.cash = m.cash
      me.frozen = m.frozen
      me.available = m.available
    }
    soon()
  })

  return { positions, orders, trades, ledger, refreshPositions, refreshOrders, refreshTrades, refreshLedger, cancel, clear }
})
