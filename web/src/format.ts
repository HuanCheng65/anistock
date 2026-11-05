// Number, percent and time formatting (docs/copywriting.md §6).
// Negatives use U+2212; up = "+", flat has no sign. Large counts use 万 / 亿; exact amounts use commas.

export const MINUS = '−'
const DASH = '–'

function fixed(v: number, digits: number): string {
  return Math.abs(v).toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits })
}

/** Exact amount: 84,213.50 (negatives with −). */
export function money(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return DASH
  const s = fixed(v, digits)
  return v < 0 && s !== fixed(0, digits) ? MINUS + s : s
}

/** Signed amount: +2,318.40 / −1.20 / 0.00 */
export function signed(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return DASH
  const s = fixed(v, digits)
  if (s === fixed(0, digits)) return s
  return (v > 0 ? '+' : MINUS) + s
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

/** The trading day changes at 04:00 (server game.toml `day_rollover`, also in /api/status as `rollover`):
 * the night session after midnight belongs to the previous day. */
export const ROLLOVER_H = 4

/** Today's trading day, from this device's clock (prefer the server's `status.day`). */
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

export const LEDGER_REASON: Record<string, string> = {
  dividend: '分红',
  dividend_tax: '股息税',
  commission: '佣金',
  stamp_duty: '印花税',
  interest: '存款利息',
  transfer: '财政补贴',
  grant: '初始资金',
  delisting: '退市返还',
  ipo: '新股认购缴款',
  ipo_refund: '认购退款',
  restructure: '破产重整',
  trade: '成交',
}

/** News kinds grouped into the four badges of the 资讯 page. */
export const NEWS_GROUP: Record<string, { label: string; tone: 'flash' | 'notice' | 'air' | 'close' }> = {
  vcm: { label: '快讯', tone: 'flash' },
  flash: { label: '快讯', tone: 'flash' },
  warning: { label: '公告', tone: 'notice' },
  premiere: { label: '放送', tone: 'air' },
  finale: { label: '放送', tone: 'air' },
  ipo: { label: '公告', tone: 'notice' },
  allotment: { label: '公告', tone: 'notice' },
  delisting: { label: '公告', tone: 'notice' },
  system: { label: '公告', tone: 'notice' },
  dividend: { label: '日结', tone: 'close' },
  rate: { label: '日结', tone: 'close' },
}
export function newsGroup(kind: string) {
  return NEWS_GROUP[kind] ?? { label: '公告', tone: 'notice' as const }
}

export const PHASE_LABEL: Record<string, string> = { auction: '集合竞价', continuous: '交易中', closed: '休市' }

/** Episode progress text: 5 / 12, 第 1180 集, 未开播 */
export function episodeText(aired: number, total: number | null, longRunner: boolean): string {
  if (!aired) return '未开播'
  if (longRunner || total == null) return `第 ${aired} 集`
  return `${Math.min(aired, total)} / ${total}`
}

/** Dividend-tax brackets from the server's [[maxDays, rate], …] (max days inclusive, FIFO lot age) as labelled rows. */
export const DEFAULT_DIVIDEND_TAX: [number, number][] = [[0, 0.2], [27, 0.1], [100000, 0]]
export function taxBrackets(t: [number, number][] = DEFAULT_DIVIDEND_TAX): { k: string; v: string }[] {
  const span = (d: number) => (d >= 7 && d % 7 === 0 ? `${d / 7} 周` : `${d} 天`)
  let prev = -1
  return t.map(([days, rate]) => {
    const start = prev + 1
    prev = days
    let k: string
    if (days >= 100000) k = `持有满 ${span(start)}`
    else if (start === 0 && days === 0) k = '当天买入'
    else if (start === 0) k = `持有不满 ${span(days + 1)}`
    else k = `持有 ${span(start)}到不满 ${span(days + 1)}`
    return { k, v: `${Math.round(rate * 100)}%` }
  })
}
