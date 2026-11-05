// The typed API v1 client (docs/api-v1.md). Types come from the server's OpenAPI document:
// `npm run gen:api` regenerates src/api/openapi.json and src/api/schema.d.ts. Amounts are integer cents
// (`Money`); format them with format.ts.
import createClient, { type Middleware } from 'openapi-fetch'
import type { components, paths } from './schema'
import { noteServerVersion } from '../version'

type S = components['schemas']
export type Money = S['Money']
export type StockRef = S['StockRef']
export type Quote = S['Quote']
export type Ratios = S['Ratios']
export type Vcm = S['Vcm']
export type BookLevel = S['BookLevel']
export type AuctionIndication = S['AuctionIndication']
export type PublicTrade = S['PublicTrade']
export type NewsItem = S['NewsItem']
export type Order = S['Order']
export type Ipo = S['Ipo']
export type Position = S['Position']
export type Me = S['Me']
export type Portfolio = S['Portfolio']
export type MarketStatus = S['MarketStatus']
export type Rules = S['Rules']
export type MarketIndex = S['MarketIndex']
export type IndicesResponse = S['IndicesResponse']
export type Airing = S['Airing']
export type MacroRow = S['MacroRow']
export type StockDetail = S['StockDetail']
export type RevenueWeek = S['RevenueWeek']
export type Candle = S['Candle']
export type IntradayPoint = S['IntradayPoint']
export type MyTrade = S['MyTrade']
export type LedgerEntry = S['LedgerEntry']
export type Leaderboard = S['Leaderboard']
export type LeaderRow = S['LeaderRow']
export type AssetDay = S['AssetDay']
export type Binding = S['Binding']
export type BindCode = S['BindCode']
export type SearchHit = S['SearchHit']
export type PlaceOrderResponse = S['PlaceOrderResponse']
export type WsServerMessage = S['WsServerMessage']
export type ErrorBody = S['ErrorBody']

/** A non-2xx answer: the server's `{code, message, details}`; `message` is shown as is. */
export class ApiError extends Error {
  status: number
  code: string
  details: Record<string, unknown>
  constructor(status: number, code: string, message: string, details: Record<string, unknown> = {}) {
    super(message)
    this.status = status
    this.code = code
    this.details = details
  }
}

const versionCheck: Middleware = {
  onResponse({ response }) {
    const v = response.headers.get('AniEx-Version')
    if (v) noteServerVersion(v)
  },
}

export const client = createClient<paths>({ baseUrl: '', credentials: 'same-origin' })
client.use(versionCheck)

type Result<T> = { data?: T; error?: unknown; response: Response }

/** Await an openapi-fetch call: its data, or an ApiError with the server's message. */
export async function call<T>(p: Promise<Result<T>>): Promise<T> {
  let r: Result<T>
  try {
    r = await p
  } catch {
    throw new ApiError(0, 'network', '无法连接服务器，检查网络后重试')
  }
  if (!r.response.ok) {
    const e = r.error as Partial<ErrorBody> | undefined
    if (e && typeof e.message === 'string' && typeof e.code === 'string') {
      throw new ApiError(r.response.status, e.code, e.message, (e.details as Record<string, unknown>) ?? {})
    }
    throw new ApiError(r.response.status, 'internal', r.response.status >= 500 ? '服务器暂时无法响应，稍后重试' : '无法完成操作，稍后重试')
  }
  return r.data as T
}

export const api = client

/** The message to show for any thrown value. */
export function errorText(e: unknown, fallback = '无法完成操作，稍后重试'): string {
  return e instanceof ApiError ? e.message : fallback
}
