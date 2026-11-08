// The signed-in player's positions, orders, trades, cash ledger, asset curve and QQ binding.
import { defineStore } from 'pinia'
import { ref } from 'vue'
import { api, call, type AssetDay, type Binding, type LedgerEntry, type MyTrade, type Order, type Position } from '../api'
import { money, qty } from '../format'
import { onLive } from '../live'
import { useSession } from './session'
import { useToast } from './toast'

export const useAccount = defineStore('account', () => {
  const positions = ref<Position[] | null>(null)
  const orders = ref<Order[] | null>(null)
  const trades = ref<MyTrade[] | null>(null)
  const tradesNext = ref<string | null>(null)
  const ledger = ref<LedgerEntry[] | null>(null)
  const ledgerNext = ref<string | null>(null)
  const history = ref<AssetDay[] | null>(null)
  const bindings = ref<Binding[] | null>(null)

  async function refreshPositions() {
    positions.value = (await call(api.GET('/api/v1/me/portfolio'))).items
  }
  async function refreshOrders() {
    orders.value = (await call(api.GET('/api/v1/me/orders', { params: { query: { status: 'open', limit: 200 } } }))).items
  }
  async function refreshTrades() {
    const r = await call(api.GET('/api/v1/me/trades', { params: { query: { limit: 50 } } }))
    trades.value = r.items
    tradesNext.value = r.next_cursor
  }
  async function moreTrades() {
    if (!tradesNext.value) return
    const r = await call(api.GET('/api/v1/me/trades', { params: { query: { limit: 50, before: tradesNext.value } } }))
    trades.value = [...(trades.value ?? []), ...r.items]
    tradesNext.value = r.next_cursor
  }
  async function refreshLedger() {
    const r = await call(api.GET('/api/v1/me/ledger', { params: { query: { limit: 50 } } }))
    ledger.value = r.items
    ledgerNext.value = r.next_cursor
  }
  async function moreLedger() {
    if (!ledgerNext.value) return
    const r = await call(api.GET('/api/v1/me/ledger', { params: { query: { limit: 50, before: ledgerNext.value } } }))
    ledger.value = [...(ledger.value ?? []), ...r.items]
    ledgerNext.value = r.next_cursor
  }
  async function refreshHistory() {
    history.value = (await call(api.GET('/api/v1/me/asset-history'))).items
  }
  async function refreshBindings() {
    bindings.value = (await call(api.GET('/api/v1/me/bindings'))).items
  }
  async function cancel(id: number) {
    const r = await call(api.DELETE('/api/v1/me/orders/{id}', { params: { path: { id } } }))
    if (orders.value) orders.value = orders.value.filter((o) => o.id !== r.order.id)
    await useSession().loadMe()
  }
  function clear() {
    positions.value = orders.value = trades.value = ledger.value = history.value = bindings.value = null
    tradesNext.value = ledgerNext.value = null
  }

  // ---- pushes on the `me` topic: own orders, fills and cash. Totals (市值, 盈亏) are the server's, so a
  // push also refetches what is already loaded, at most every 400 ms.
  let resync: number | undefined
  function soon() {
    window.clearTimeout(resync)
    resync = window.setTimeout(() => {
      useSession().loadMe().catch(() => {})
      if (positions.value) refreshPositions().catch(() => {})
    }, 400)
  }
  // fills of one order within a moment are summed into one toast
  const fills = new Map<number, { qty: number; notional: number; at: number }>()
  /** the latest own fill, summed over its order (the 首笔成交 note shows it) */
  const lastFill = ref<{ side: 'buy' | 'sell'; quantity: number; price: number } | null>(null)
  onLive('fill', (m) => {
    const t = m.trade
    if (trades.value && !trades.value.some((x) => x.id === t.id)) trades.value = [t, ...trades.value]
    const key = t.order_id ?? -1
    const now = Date.now()
    const prev = fills.get(key)
    const f = prev && now - prev.at < 3000 ? prev : { qty: 0, notional: 0, at: now }
    f.qty += t.quantity
    f.notional += t.quantity * t.price
    f.at = now
    fills.set(key, f)
    lastFill.value = { side: t.side, quantity: f.qty, price: Math.round(f.notional / f.qty) }
    useToast().show({
      key: 'fill' + key,
      text: `${t.side === 'buy' ? '买入' : '卖出'}成交 ${qty(f.qty)} 股 · ${money(Math.round(f.notional / f.qty))}`,
      sub: t.stock.title,
      tone: t.side,
    })
    soon()
  })
  onLive('order', (m) => {
    if (orders.value) {
      const rest = orders.value.filter((o) => o.id !== m.order.id)
      orders.value = m.order.status === 'open' ? [m.order, ...rest].sort((a, b) => b.id - a.id) : rest
    }
    soon()
  })
  onLive('cash', (m) => {
    const me = useSession().me
    if (me) {
      me.cash = m.cash
      me.frozen_cash = m.frozen_cash
      me.available_cash = m.available_cash
    }
    soon()
  })

  return {
    lastFill,
    positions, orders, trades, tradesNext, ledger, ledgerNext, history, bindings,
    refreshPositions, refreshOrders, refreshTrades, moreTrades, refreshLedger, moreLedger, refreshHistory, refreshBindings,
    cancel, clear,
  }
})
