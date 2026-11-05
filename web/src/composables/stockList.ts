// The 股票 table's filters and sort keys, shared with the detail page's list sidebar.
import type { MarketStock } from '../api'

export interface ListCtx {
  watch: Set<number>
  seasonStart: string
  ratio: Map<number, { pe: number | null; dividend_yield: number | null }>
}

export const FILTERS: { key: string; label: string; test: (s: MarketStock, c: ListCtx) => boolean }[] = [
  { key: 'watch', label: '自选', test: (s, c) => c.watch.has(s.id) },
  { key: 'all', label: '全部', test: () => true },
  { key: 'season', label: '本季新番', test: (s, c) => !s.long_runner && s.first_air >= c.seasonStart },
  { key: 'long', label: '长篇连载', test: (s) => s.long_runner },
  { key: 'done', label: '已完结', test: (s) => !s.long_runner && s.episodes != null && s.aired >= s.episodes },
  { key: 'new', label: '新上市', test: (s) => !!s.new },
  { key: 'cooling', label: '冷静期中', test: (s) => !!s.cooling },
]

export type SortKey = 'price' | 'change' | 'turnover' | 'cap' | 'pe' | 'dy' | 'rev' | 'eps'
export const SORTS: { key: SortKey; label: string }[] = [
  { key: 'price', label: '现价' },
  { key: 'change', label: '涨跌幅' },
  { key: 'turnover', label: '成交额' },
  { key: 'cap', label: '市值' },
  { key: 'pe', label: '市盈率' },
  { key: 'dy', label: '股息率' },
  { key: 'rev', label: '本周营收' },
  { key: 'eps', label: '集数' },
]

export function sortValue(s: MarketStock, k: SortKey, c: ListCtx): number | null {
  switch (k) {
    case 'price': return s.price
    case 'change': return s.change
    case 'turnover': return s.turnover
    case 'cap': return s.market_cap
    case 'pe': return c.ratio.get(s.id)?.pe ?? null
    case 'dy': return c.ratio.get(s.id)?.dividend_yield ?? null
    case 'rev': return s.week_revenue
    case 'eps': return s.aired
  }
}

export function listRows(all: MarketStock[], filter: string, sort: SortKey, desc: boolean, c: ListCtx): MarketStock[] {
  const f = FILTERS.find((x) => x.key === filter) ?? FILTERS[1]
  const rows = all.filter((s) => f.test(s, c))
  // missing values sort last in either direction
  return rows.sort((a, b) => {
    const x = sortValue(a, sort, c), y = sortValue(b, sort, c)
    if (x == null && y == null) return a.code < b.code ? -1 : 1
    if (x == null) return 1
    if (y == null) return -1
    return desc ? y - x : x - y
  })
}
