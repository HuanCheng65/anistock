// Number formatting in Chinese conventions.

export function money(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return '–'
  return v.toLocaleString('zh-CN', { minimumFractionDigits: digits, maximumFractionDigits: digits })
}

/** Large amounts as 万 / 亿 */
export function big(v: number | null | undefined): string {
  if (v == null || Number.isNaN(v)) return '–'
  const a = Math.abs(v)
  if (a >= 1e8) return (v / 1e8).toFixed(2) + '亿'
  if (a >= 1e4) return (v / 1e4).toFixed(a >= 1e6 ? 0 : 1) + '万'
  return v.toFixed(0)
}

export function pct(v: number | null | undefined, digits = 2, sign = true): string {
  if (v == null || Number.isNaN(v)) return '–'
  const s = (v * 100).toFixed(digits) + '%'
  return sign && v > 0 ? '+' + s : s
}

export function signed(v: number | null | undefined, digits = 2): string {
  if (v == null || Number.isNaN(v)) return '–'
  return (v > 0 ? '+' : '') + money(v, digits)
}

export function dir(v: number | null | undefined): 'up' | 'down' | 'flat' {
  if (v == null || Math.abs(v) < 1e-12) return 'flat'
  return v > 0 ? 'up' : 'down'
}

export function episodes(aired: number, total: number | null, longRunner: boolean): string {
  if (longRunner || total == null) return aired ? `第 ${aired} 集` : '未开播'
  if (!aired) return `未开播 · 共 ${total} 集`
  if (aired >= total) return `已完结 · ${total} 集`
  return `${aired} / ${total} 集`
}

export function time(ts: string): string {
  // "2026-11-05T10:00:00+08:00" -> "11-05 10:00"
  return ts.slice(5, 16).replace('T', ' ')
}

export function shortDate(d: string): string {
  return d.slice(5)
}

export function airingIn(epochSeconds: number): string {
  const ms = epochSeconds * 1000 - Date.now()
  const d = new Date(epochSeconds * 1000)
  const when = `${d.getMonth() + 1}月${d.getDate()}日 ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
  if (ms <= 0) return when
  const h = Math.floor(ms / 3600000)
  if (h < 24) return `${when}（${h} 小时后）`
  return `${when}（${Math.floor(h / 24)} 天后）`
}

export const LEDGER_REASON: Record<string, string> = {
  dividend: '分红',
  dividend_tax: '股息税',
  commission: '佣金',
  stamp_duty: '印花税',
  interest: '存款利息',
  transfer: '财政补贴',
  grant: '初始资金',
  delisting: '退市返还',
  ipo: '新股缴款',
  ipo_refund: '认购退款',
}

export const NEWS_KIND: Record<string, string> = {
  ipo: '新股认购',
  premiere: '开播',
  finale: '完结',
  dividend: '分红',
  rate: '央行',
  delisting: '退市',
  vcm: '冷静期',
  allotment: '配售结果',
  system: '公告',
}

export function hhmm(ts: string | null | undefined): string {
  return ts ? ts.slice(11, 16) : '–'
}

export const PHASE_LABEL: Record<string, string> = { auction: '集合竞价', continuous: '交易中', closed: '休市' }
