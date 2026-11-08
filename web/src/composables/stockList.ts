// The 股票 table's filters and sort keys, shared with the detail page's list sidebar.
import type { Quote } from '../api'

export interface ListCtx {
  watch: Set<number>
  seasonStart: string
}

export const FILTERS: { key: string; label: string; test: (s: Quote, c: ListCtx) => boolean }[] = [
  { key: 'watch', label: '自选', test: (s, c) => c.watch.has(s.stock.id) },
  { key: 'all', label: '全部', test: () => true },
  { key: 'season', label: '本季新番', test: (s, c) => !s.is_long_runner && s.first_air >= c.seasonStart },
  { key: 'past', label: '往季番剧', test: (s, c) => !s.is_long_runner && s.first_air < c.seasonStart },
  { key: 'long', label: '长篇连载', test: (s) => s.is_long_runner },
  { key: 'done', label: '已完结', test: (s) => !s.is_long_runner && s.episodes != null && s.episodes_aired >= s.episodes },
  { key: 'new', label: '新上市', test: (s) => s.is_new },
  { key: 'cooling', label: '冷静期中', test: (s) => s.vcm_active },
]

export type SortKey = 'price' | 'change' | 'turnover' | 'cap' | 'pe' | 'dy' | 'rev' | 'eps'
export const SORTS: { key: SortKey; label: string }[] = [
  { key: 'price', label: '现价' },
  { key: 'change', label: '涨跌幅' },
  { key: 'turnover', label: '成交额' },
  { key: 'cap', label: '市值' },
  { key: 'pe', label: '市盈率(TTM)' },
  { key: 'dy', label: '股息率(TTM)' },
  { key: 'rev', label: '本周营收' },
  { key: 'eps', label: '集数' },
]

export function sortValue(s: Quote, k: SortKey): number | null {
  switch (k) {
    case 'price': return s.price
    case 'change': return s.change
    case 'turnover': return s.turnover
    case 'cap': return s.market_cap
    case 'pe': return s.ratios.pe_ttm
    case 'dy': return s.ratios.dividend_yield_ttm
    case 'rev': return s.week_revenue
    case 'eps': return s.episodes_aired
  }
}

export function listRows(all: Quote[], filter: string, sort: SortKey, desc: boolean, c: ListCtx): Quote[] {
  const f = FILTERS.find((x) => x.key === filter) ?? FILTERS[1]
  const rows = all.filter((s) => f.test(s, c))
  // missing values sort last in either direction
  return rows.sort((a, b) => {
    const x = sortValue(a, sort), y = sortValue(b, sort)
    if (x == null && y == null) return a.stock.code < b.stock.code ? -1 : 1
    if (x == null) return 1
    if (y == null) return -1
    return desc ? y - x : x - y
  })
}
