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
    throw new ApiError(0, '无法连接服务器，检查网络后重试')
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
    let msg = '无法完成操作，稍后重试'
    const d = data?.detail
    if (typeof d === 'string') msg = rewrite(d)
    else if (Array.isArray(d) && d.length) msg = translateValidation(d[0])
    else if (res.status >= 500) msg = '服务器暂时无法响应，稍后重试'
    throw new ApiError(res.status, msg)
  }
  return data as T
}

/** The server's messages are fixed by the v1 API; show them in the site's wording (docs/copywriting.md). */
const MESSAGES: Record<string, string> = {
  '邀请码不对，请向社团管理员索取': '邀请码不正确，可以向社团管理员索取',
  '用户名或密码不对': '用户名或密码不正确',
  '这个用户名已经有人用了': '用户名已有人使用',
  '请先登录': '登录后可用',
  '价格格式不对': '价格格式不正确',
  '价格最多两位小数，且必须大于 0': '价格需大于 0，最多两位小数',
  '找不到这笔委托': '委托不存在',
  '这笔委托已经结束，不能撤单': '委托已结束，无法撤单',
  '集合竞价阶段只接受限价单': '集合竞价时段只接受限价委托',
  '可卖股数不足': '可卖数量不足',
  '认购已经截止，不能撤回': '认购已截止，无法撤回',
  '这支新股不在认购期': '不在认购期',
  '没有这支股票': '股票不存在',
  '没有这个接口': '无法完成操作，稍后重试',
  '买卖方向不对': '买卖方向不正确',
  '委托类型不对': '委托类型不正确',
  '限价单需要价格': '限价委托需要填写价格',
}
function rewrite(m: string): string {
  if (MESSAGES[m]) return MESSAGES[m]
  if (m.startsWith('现在休市')) return m.replace('现在休市', '休市中')
  if (m.startsWith('价格太离谱了')) return m.replace('价格太离谱了', '价格超出范围')
  return m
}

function translateValidation(e: any): string {
  const field = Array.isArray(e?.loc) ? e.loc[e.loc.length - 1] : ''
  const names: Record<string, string> = { username: '用户名', password: '密码', qty: '数量', price: '价格', invite: '邀请码' }
  const name = names[field] ?? '输入内容'
  if (field === 'username' && (e?.type === 'string_too_short' || e?.type === 'string_too_long')) return '用户名需为 2–20 个字符'
  if (field === 'password' && e?.type === 'string_too_short') return '密码至少 6 位'
  if (e?.type === 'string_too_short') return `${name}太短`
  if (e?.type === 'string_too_long') return `${name}太长`
  if (e?.type?.startsWith('greater_than')) return `${name}需大于 0`
  return `${name}格式不正确`
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
  /** the trading day changes here (04:00): the night session belongs to the previous day */
  rollover?: string
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

// ---------------------------------------------------------------- v2 (read-only, outside v1)

export interface IndexSeries {
  key: 'all' | 'season' | 'long'
  name: string
  members: number
  base_day: string
  value: number
  prev: number
  change: number
  series: [string, number][]
}

export interface Indices {
  indices: IndexSeries[]
  breadth: { up: number; flat: number; down: number }
  season_start: string
}

export interface Airing {
  company_id: number
  episode: number
  airing_at: string
  aired: boolean
}

export interface Profile {
  episodes: { n: number; day: string }[]
  next_episode: { episode: number; airing_at: string } | null
  dividends: { day: string; ps: number }[]
  ratios: { pe: number | null; dividend_yield: number | null; pb: number | null; pe_dyn: number | null; dividend_yield_dyn: number | null }
  holders: { committee: number; npc: number; player: number; unissued: number }
  shares: number
}

export interface Listing {
  id: number
  title: string
  listed_on: string
  ipo_price: number
  first_close: number | null
  first_change: number | null
  oversubscribed: number | null
  ratio: number | null
}

export interface Ratios {
  rows: { id: number; pe: number | null; dividend_yield: number | null; pe_dyn: number | null; dividend_yield_dyn: number | null }[]
  season_start: string
}

export interface MacroSeries {
  revenue_week: [string, number][]
  demand: [string, number][]
}

export interface Listings {
  recent: Listing[]
  upcoming: { id: number; title: string; sub_start: string; first_air: string }[]
}

export interface IntradayPoint {
  minute: string
  price: number
  volume: number
}
