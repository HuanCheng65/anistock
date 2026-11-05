// The list a stock was opened from, for the detail page's sidebar, ↑/↓ keys and the phone ‹ › buttons.
import { computed, ref, watch, type Ref } from 'vue'
import type { Quote } from '../api'
import { useAccount } from '../stores/account'
import { useMarket } from '../stores/market'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

export function useStockNav(current: Ref<number>) {
  const market = useMarket()
  const prefs = usePrefs()
  const account = useAccount()
  const session = useSession()

  const ctx = computed(() => prefs.context)
  const tabs = computed(() => {
    const out = [{ key: 'watch', label: '自选' }]
    if (session.me) out.push({ key: 'hold', label: '持仓' })
    const c = ctx.value
    out.push({ key: 'ctx', label: c && c.key !== 'watch' && c.key !== 'hold' ? c.label : '全部' })
    return out
  })
  const initial = () => {
    const c = ctx.value
    if (c?.key === 'watch' && prefs.watchlist.length) return 'watch'
    if (c?.key === 'hold' && session.me) return 'hold'
    if (c) return 'ctx'
    return prefs.watchlist.includes(current.value) ? 'watch' : 'ctx'
  }
  const tab = ref(initial())

  const pick = (ids: number[]) => ids.map((id) => market.byId.get(id)).filter((s): s is Quote => !!s)
  const rows = computed<Quote[]>(() => {
    if (tab.value === 'watch') return pick(prefs.watchlist)
    if (tab.value === 'hold') return pick((account.positions ?? []).map((p) => p.stock.id))
    const c = ctx.value
    if (c && c.key !== 'watch' && c.key !== 'hold') return pick(c.ids)
    if (c?.key === 'watch' || c?.key === 'hold') return pick(c.ids)
    return [...(market.stocks ?? [])].sort((a, b) => b.change - a.change)
  })
  // keep the holdings list fresh enough when that tab is in use
  watch(tab, (t) => t === 'hold' && session.me && account.refreshPositions().catch(() => {}), { immediate: true })

  const index = computed(() => rows.value.findIndex((s) => s.stock.id === current.value))
  const prev = computed(() => (index.value > 0 ? rows.value[index.value - 1] : null))
  const next = computed(() => (index.value >= 0 && index.value < rows.value.length - 1 ? rows.value[index.value + 1] : null))
  return { tabs, tab, rows, prev, next, index }
}
