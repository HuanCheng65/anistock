// API client and response types. Money fields are 币 (floats); the server converts from cents.

export class ApiError extends Error {
  status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  let res: Response
  try {
    res = await fetch(path, {
      method,
      credentials: 'same-origin',
      headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    })
  } catch {
    throw new ApiError(0, '连不上服务器，请检查网络后重试')
  }
  let data: any = null
  const text = await res.text()
  if (text) {
    try {
      data = JSON.parse(text)
    } catch {
      data = null
    }
  }
  if (!res.ok) {
    let msg = '请求失败'
    const d = data?.detail
    if (typeof d === 'string') msg = d
    else if (Array.isArray(d) && d.length) msg = translateValidation(d[0])
    else if (res.status >= 500) msg = '服务器出错了，请稍后再试'
    throw new ApiError(res.status, msg)
  }
  return data as T
}

function translateValidation(e: any): string {
  const field = Array.isArray(e?.loc) ? e.loc[e.loc.length - 1] : ''
  const names: Record<string, string> = { username: '用户名', password: '密码', qty: '数量', price: '价格', invite: '邀请码' }
  const name = names[field] ?? '输入'
  if (e?.type === 'string_too_short') return `${name}太短`
  if (e?.type === 'string_too_long') return `${name}太长`
  if (e?.type?.startsWith('greater_than')) return `${name}必须大于 0`
  return `${name}格式不对`
}

export const api = {
  get: <T>(p: string) => request<T>('GET', p),
  post: <T>(p: string, b?: unknown) => request<T>('POST', p, b ?? {}),
  del: <T>(p: string) => request<T>('DELETE', p),
}

// ---------------------------------------------------------------- types

export interface Me {
  id: number
  username: string
  cash: number
  frozen: number
  available: number
  stock_value: number
  total: number
  initial: number
  return_pct: number
  dividends: number
  deposit_rate: number
}

export interface Status {
  now: string
  day: string
  trading: boolean
  phase: 'auction' | 'continuous' | 'closed'
  auction_start: string
  open: string
  close: string
  last_closed: string
  launched_on: string
  rate: number
  deposit_rate: number
  rules: {
    vcm_threshold: number
    vcm_window_min: number
    vcm_cool_min: number
    commission: number
    stamp_duty: number
    dividend_tax: [number, number][]
    initial_cash: number
  }
}

export interface MarketStock {
  id: number
  code: string
  title: string
  title_ja?: string | null
  title_en: string | null
  price: number
  prev_close: number
  change: number
  volume: number
  turnover: number
  market_cap: number
  week_revenue: number
  revenue_change: number | null
  aired: number
  episodes: number | null
  long_runner: boolean
  first_air: string
  listed_on: string
  ipo_left: number
  ipo_price: number
  new?: boolean
  cooling?: boolean
  spark: number[]
}

export interface BookLevel {
  price: number
  qty: number
  orders: number
}

export interface Position {
  company_id: number
  code: string
  title: string
  qty: number
  available: number
  avg_cost: number
  price: number
  value: number
  pnl: number
  pnl_pct: number | null
  dividends: number
  day_change: number
  oldest_lot: string | null
}

export interface Order {
  id: number
  company_id: number
  side: 'buy' | 'sell'
  kind: 'limit' | 'market'
  price: number | null
  qty: number
  filled: number
  status: 'open' | 'filled' | 'cancelled'
  created_at: string
  updated_at: string
  title?: string
  code?: string
}

export interface WeekRow {
  week: string
  revenue: number
  us: number
  cn: number
  jp: number
  dividend_ps: number
  aired: number
}

export interface NewsItem {
  id: number
  day: string
  ts?: string
  kind: string
  title: string
  body: string | null
  company_id?: number | null
  code?: string | null
}

export interface StockDetail {
  id: number
  code: string
  title: string
  title_ja?: string | null
  title_en: string | null
  studio: string | null
  status: 'listed' | 'delisted' | 'subscribing' | string
  episodes: number | null
  long_runner: boolean
  first_air: string
  listed_on: string
  shares: number
  ipo_price: number
  ipo_left?: number
  quote: {
    price: number
    prev_close: number
    open: number | null
    high: number | null
    low: number | null
    volume: number
    turnover: number
    open_ref: number | null
  }
  vcm?: { active: boolean; until: string | null; low: number | null; high: number | null; count: number }
  auction?: { price: number | null; volume: number } | null
  ipo?: Ipo | null
  book: { bids: BookLevel[]; asks: BookLevel[] }
  trades: { price: number; qty: number; ts: string }[]
  weeks: WeekRow[]
  book_value_ps: number | null
  holders: Record<string, number>
  audiences: {
    anilist?: { popularity: number | null; score: number | null; next_episode: { episode: number; airingAt: number } | null; genres?: string[] }
    bangumi?: { collection: Record<string, number> | null; score: number | null; name_cn: string | null }
    annict?: { watchers: number | null }
  }
  news: NewsItem[]
  position?: Position | null
  my_orders?: Order[]
  available_cash?: number
}

export interface Candle {
  time: string
  open: number
  high: number
  low: number
  close: number
  volume: number
  replay: boolean
  live?: boolean
}

export interface PlaceResult {
  order_id: number
  status: 'open' | 'filled' | 'cancelled'
  filled: number
  avg_price: number | null
}

export interface Trade {
  id: number
  company_id: number
  code: string
  title: string
  side: 'buy' | 'sell'
  price: number
  qty: number
  ts: string
}

export interface LedgerEntry {
  ts: string
  reason: string
  title: string | null
  amount: number
}

export interface LeaderRow {
  rank: number
  username: string
  total: number
  return_pct: number
}

export interface MacroDay {
  day: string
  rate: number
  m2: number
  m2_growth: number | null
  injected: number
  exported: number
  taxes: number
}

export interface Ipo {
  id: number
  code: string
  title: string
  title_ja?: string | null
  ipo_price: number
  sub_start: string
  sub_end: string
  listed_on: string
  first_air: string
  float_qty: number
  base_public_qty: number
  applied_qty: number
  applicants: number
  status: 'open' | 'allotted'
  my: { qty: number; allotted: number | null } | null
  result: { oversubscribed: number; public_qty: number; ratio: number } | null
}
