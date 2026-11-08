// Number, percent and time formatting (docs/copywriting.md §6).
// Negatives use U+2212; up = "+", flat has no sign. Large counts use 万 / 亿; exact amounts use commas.
// Amounts from the API are integer cents (`Money`): money(), signed() and bigMoney() take cents; num()
// and signedNum() take plain numbers (index points).

export const MINUS = '−'
const DASH = '–'

function fixed(v: number, digits: number): string {
  return Math.abs(v).toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits })
}

/** A plain number with fixed decimals: 1,043.27 (negatives with −). */
export function num(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return DASH
  const s = fixed(v, digits)
  return v < 0 && s !== fixed(0, digits) ? MINUS + s : s
}

/** A signed plain number: +4.37 / −1.20 / 0.00 */
export function signedNum(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return DASH
  const s = fixed(v, digits)
  if (s === fixed(0, digits)) return s
  return (v > 0 ? '+' : MINUS) + s
}

/** Exact amount from cents: 8421350 -> 84,213.50 */
export function money(cents: number | null | undefined, digits = 2): string {
  return cents == null ? DASH : num(cents / 100, digits)
}

/** Signed amount from cents: +2,318.40 / −1.20 / 0.00 */
export function signed(cents: number | null | undefined, digits = 2): string {
  return cents == null ? DASH : signedNum(cents / 100, digits)
}

/** Compact amount from cents: 1.2万, 3.4亿 */
export function bigMoney(cents: number | null | undefined): string {
  return cents == null ? DASH : big(cents / 100)
}

/** 币 text typed by the player -> cents; null unless a positive number with at most two decimals. */
export function toCents(text: string): number | null {
  const t = text.trim()
  if (!/^\d+(\.\d{1,2})?$/.test(t)) return null
  const c = Math.round(Number(t) * 100)
  return c > 0 ? c : null
}

/** Cents as the text of a price input: 22045 -> "220.45" */
export function centsText(cents: number): string {
  return (cents / 100).toFixed(2)
}

/** Ratio as signed percent: 0.0961 -> +9.61% */
export function pct(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return DASH
  const s = Math.abs(v * 100).toFixed(digits)
  if (Number(s) === 0) return s + '%'
  return (v > 0 ? '+' : MINUS) + s + '%'
}

/** Ratio as unsigned percent: 0.46 -> 46% */
export function share(v: number | null | undefined, digits = 0): string {
  if (v == null || Number.isNaN(v)) return DASH
  return (v * 100).toFixed(digits) + '%'
}

/** Integer count with commas: 1,824 */
export function qty(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return DASH
  return (v < 0 ? MINUS : '') + Math.abs(Math.round(v)).toLocaleString('en-US')
}

/** Compact: under 1万 as is, then 1.2万, 1.2亿 */
export function big(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return DASH
  const a = Math.abs(v)
  const sign = v < 0 ? MINUS : ''
  if (a >= 1e8) return sign + trim1(a / 1e8) + '亿'
  if (a >= 1e4) return sign + trim1(a / 1e4) + '万'
  return sign + Math.round(a).toLocaleString('en-US')
}
function trim1(x: number): string {
  return x >= 1000 ? Math.round(x).toLocaleString('en-US') : x.toFixed(1)
}

export type Dir = 'up' | 'down' | 'flat'
export function dir(v: number | null | undefined): Dir {
  if (v == null || Math.abs(v) < 5e-5) return 'flat'
  return v > 0 ? 'up' : 'down'
}

// ---------------------------------------------------------------- time

/** Parse "2026-11-05T21:14:08+08:00". */
function parse(ts: string): Date {
  return new Date(ts)
}
const pad = (n: number) => String(n).padStart(2, '0')

/** Game-local wall clock pieces straight from the ISO string (the server sends local offsets). */
function parts(ts: string) {
  return { y: ts.slice(0, 4), mo: ts.slice(5, 7), d: ts.slice(8, 10), hm: ts.slice(11, 16), hms: ts.slice(11, 19) }
}

/** Trade time: HH:mm:ss */
export function hms(ts: string | null | undefined): string {
  return ts ? parts(ts).hms : DASH
}

/** HH:mm */
export function hm(ts: string | null | undefined): string {
  return ts ? parts(ts).hm : DASH
}

/** §6: 刚刚 / N 秒前 / N 分钟前 (same hour) / 今天 HH:mm / MM-dd HH:mm / yyyy-MM-dd HH:mm */
export function when(ts: string | null | undefined, today?: string): string {
  if (!ts) return DASH
  const t = parse(ts).getTime()
  const diff = (Date.now() - t) / 1000
  if (diff >= 0 && diff < 10) return '刚刚'
  if (diff >= 0 && diff < 60) return `${Math.floor(diff)} 秒前`
  if (diff >= 0 && diff < 3600) return `${Math.floor(diff / 60)} 分钟前`
  const p = parts(ts)
  const day = tradingDayOf(ts)
  const now = today ?? tradingDay()
  if (day === now) return `今天 ${p.hm}`
  if (p.y === now.slice(0, 4)) return `${p.mo}-${p.d} ${p.hm}`
  return `${day} ${p.hm}`
}

/** Day label for grouping: 今天 / 昨天 / MM-dd / yyyy-MM-dd */
export function dayLabel(day: string, today?: string): string {
  const now = today ?? tradingDay()
  if (day === now) return '今天'
  if (day === addDays(now, -1)) return '昨天'
  if (day.slice(0, 4) === now.slice(0, 4)) return day.slice(5)
  return day
}

/** Short day for lists: 今天 / MM-dd */
export function shortDay(day: string, today?: string): string {
  const now = today ?? tradingDay()
  if (day === now) return '今天'
  if (day.slice(0, 4) === now.slice(0, 4)) return day.slice(5)
  return day
}

export function localDay(d = new Date()): string {
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
}

/** The trading day changes at 04:00 (server game.toml `day_rollover`, also in /api/v1/rules):
 * the night session after midnight belongs to the previous day. */
export const ROLLOVER_H = 4

/** Today's trading day, from this device's clock (prefer the server's `status.trading_day`). */
export function tradingDay(now = new Date()): string {
  return localDay(new Date(now.getTime() - ROLLOVER_H * 3600_000))
}

/** Trading day of a game timestamp (`2026-11-06T01:30:00+08:00` -> `2026-11-05`), by its own wall time. */
export function tradingDayOf(ts: string): string {
  const h = Number(ts.slice(11, 13))
  return h < ROLLOVER_H ? addDays(ts.slice(0, 10), -1) : ts.slice(0, 10)
}

/** Continuous trading hours: 09:00–23:00, or 09:00 至次日 04:00 when the session runs past midnight. */
export function sessionText(open = '09:00', close = '04:00'): string {
  return close < open ? `${open} 至次日 ${close}` : `${open}–${close}`
}

export function addDays(day: string, n: number): string {
  const d = new Date(day + 'T12:00:00')
  d.setDate(d.getDate() + n)
  return localDay(d)
}

const WEEK = ['日', '一', '二', '三', '四', '五', '六']
/** 11 月 5 日 星期四 */
export function longDate(day: string): string {
  const d = new Date(day + 'T12:00:00')
  return `${d.getMonth() + 1} 月 ${d.getDate()} 日 星期${WEEK[d.getDay()]}`
}

/** 周四 22:00 from an ISO timestamp */
export function weekdayTime(ts: string): string {
  const d = new Date(ts.slice(0, 10) + 'T12:00:00')
  return `周${WEEK[d.getDay()]} ${parts(ts).hm}`
}

// ---------------------------------------------------------------- labels

/** The news groups (the server's `category`) as badges and filter chips. `media`: external news (docs/design/news.md),
 * published as news items from [news] mode = publish. */
export const NEWS_CATEGORY: Record<string, { label: string; tone: 'flash' | 'notice' | 'air' | 'close' | 'media'; chip?: false }> = {
  flash: { label: '快讯', tone: 'flash' },
  notice: { label: '公告', tone: 'notice' },
  airing: { label: '放送', tone: 'air' },
  close: { label: '日结', tone: 'close' },
  media: { label: '媒体', tone: 'media' },
}
export function newsGroup(category: string) {
  return NEWS_CATEGORY[category] ?? NEWS_CATEGORY.notice
}

export const PHASE_LABEL: Record<string, string> = { auction: '集合竞价', continuous: '交易中', closed: '休市' }

/** Episode progress text: 5 / 12, 第 1180 集, 未开播 */
export function episodeText(aired: number, total: number | null, longRunner: boolean): string {
  if (!aired) return '未开播'
  if (longRunner || total == null) return `第 ${aired} 集`
  return `${Math.min(aired, total)} / ${total}`
}

/** Dividend-tax brackets from /rules (`max_holding_days` inclusive, null = no limit; FIFO lot age) as labelled rows. */
export const DEFAULT_DIVIDEND_TAX = [
  { max_holding_days: 0, rate: 0.2 },
  { max_holding_days: 27, rate: 0.1 },
  { max_holding_days: null, rate: 0 },
] as { max_holding_days: number | null; rate: number }[]
export function taxBrackets(t: { max_holding_days: number | null; rate: number }[] = DEFAULT_DIVIDEND_TAX): { k: string; v: string }[] {
  const span = (d: number) => (d >= 7 && d % 7 === 0 ? `${d / 7} 周` : `${d} 天`)
  let prev = -1
  return t.map(({ max_holding_days, rate }) => {
    const days = max_holding_days ?? Infinity
    const start = prev + 1
    prev = days
    let k: string
    if (max_holding_days == null) k = `持有满 ${span(start)}`
    else if (start === 0 && days === 0) k = '当天买入'
    else if (start === 0) k = `持有不满 ${span(days + 1)}`
    else k = `持有 ${span(start)}到不满 ${span(days + 1)}`
    return { k, v: `${Math.round(rate * 100)}%` }
  })
}

/** Days until the newest lot bought on `newest` reaches the lowest dividend tax band (0: already there, or no
 *  data). Lots are taxed first in, first out, so once the newest is through, every share is. */
export function lowestTaxIn(newest: string | null | undefined, today: string | undefined,
  t: { max_holding_days: number | null; rate: number }[] = DEFAULT_DIVIDEND_TAX): number {
  if (!newest || !today || !t.length) return 0
  const floor = Math.min(...t.map((b) => b.rate))
  const taxed = t.filter((b) => b.rate > floor && b.max_holding_days != null).map((b) => b.max_holding_days as number)
  if (!taxed.length) return 0
  const held = Math.round((Date.parse(today) - Date.parse(newest)) / 86400000)
  return Math.max(0, Math.max(...taxed) + 1 - held)
}
