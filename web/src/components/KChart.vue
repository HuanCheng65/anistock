<script setup lang="ts">
// K-line (day / week / month) or intraday line, with per-episode air markers (dashed line + label) and
// dividend markers (diamonds) drawn in an overlay that follows the chart's time scale.
// Up = hollow red, down = solid green. Replay days (before the market opened) are drawn faded, and a thin
// rule marks where real trading starts. Volume sits on its own hidden overlay scale in the bottom fifth; the
// price axis labels only the price range (no labels in the volume band).
// Data arrive in cents; the chart's price scale is in 币 (cents / 100), the readout formats cents.
import {
  createChart, CrosshairMode, LineStyle,
  type IChartApi, type ISeriesApi, type IPriceLine, type Time, type UTCTimestamp,
} from 'lightweight-charts'
import { computed, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue'
import type { Candle, IntradayPoint } from '../api'
import { cssVar, useThemeTick } from '../composables/useTheme'
import { money, qty } from '../format'

export type Period = 'intraday' | 'day' | 'week' | 'month'

const props = defineProps<{
  candles: Candle[]
  period: Period
  intraday?: IntradayPoint[]
  prevClose?: number
  episodes?: { episode: number; trading_day: string }[]
  dividends?: { trading_day: string; per_share: number }[]
  wide?: boolean
  /** trading and receiving pushes: the intraday line's last point pulses */
  live?: boolean
}>()

const host = ref<HTMLDivElement>()
const chart = shallowRef<IChartApi | null>(null)
let candle: ISeriesApi<'Candlestick'> | null = null
let vol: ISeriesApi<'Histogram'> | null = null
let line: ISeriesApi<'Line'> | null = null
let prevLine: IPriceLine | null = null
const tick = useThemeTick()
const hover = ref<{ t: string; o?: number; h?: number; l?: number; c: number; v: number } | null>(null)
const marks = ref<{ x: number; label: string; key: string }[]>([])
const divs = ref<{ x: number; key: string; ps: number }[]>([])
const width = ref(0)
const dot = ref<{ x: number; y: number; up: boolean | null } | null>(null)
/** x of the boundary between the last replay bar and the first live bar, if both are on screen */
const launchX = ref<number | null>(null)
/** price-axis labels below this (币) are left blank: that band belongs to the volume bars */
let axisFloor = -Infinity
const fmtPrice = (p: number) => (p < axisFloor ? '' : p.toFixed(2))

function hexA(hex: string, a: number) {
  const m = hex.replace('#', '')
  if (m.length !== 6) return hex
  const n = parseInt(m, 16)
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`
}

function bucket(day: string, p: Period): string {
  if (p === 'month') return day.slice(0, 7) + '-01'
  if (p === 'week') {
    const d = new Date(day + 'T12:00:00Z')
    const wd = (d.getUTCDay() + 6) % 7
    d.setUTCDate(d.getUTCDate() - wd)
    return d.toISOString().slice(0, 10)
  }
  return day
}

/** a bar in cents, keyed by its first trading day */
interface Bar { time: string; open: number; high: number; low: number; close: number; volume: number; replay: boolean }
const toBar = (k: Candle): Bar => ({
  time: k.trading_day, open: k.open, high: k.high, low: k.low, close: k.close, volume: k.volume, replay: k.source === 'replay',
})
const yuan = (c: number) => c / 100

const bars = computed<Bar[]>(() => {
  if (props.period === 'day' || props.period === 'intraday') return props.candles.map(toBar)
  const out: Bar[] = []
  for (const c of props.candles) {
    const k = toBar(c)
    const t = bucket(k.time, props.period)
    const last = out[out.length - 1]
    if (last && last.time === t) {
      last.high = Math.max(last.high, k.high)
      last.low = Math.min(last.low, k.low)
      last.close = k.close
      last.volume += k.volume
      last.replay = last.replay && k.replay
    } else out.push({ ...k, time: t })
  }
  return out
})

const minuteTime = (m: string) => (Date.parse(m.slice(0, 16) + ':00Z') / 1000) as UTCTimestamp

function colors() {
  return {
    up: cssVar('--up'), down: cssVar('--down'), grid: cssVar('--grid'), ground: cssVar('--ground'),
    muted: cssVar('--muted'), brand: cssVar('--brand'), base: cssVar('--base-line'), ink2: cssVar('--ink-2'),
  }
}

function draw() {
  const ch = chart.value
  if (!ch || !candle || !vol || !line) return
  const c = colors()
  ch.applyOptions({
    layout: { background: { color: 'transparent' }, textColor: c.muted, fontSize: 10, fontFamily: cssVar('--font-num') },
    grid: { vertLines: { visible: false }, horzLines: { color: c.grid } },
    rightPriceScale: { borderVisible: false },
    timeScale: { borderVisible: false, timeVisible: props.period === 'intraday', secondsVisible: false },
    crosshair: { vertLine: { color: c.base, labelBackgroundColor: c.ink2 }, horzLine: { color: c.base, labelBackgroundColor: c.ink2 } },
  })
  candle.applyOptions({
    upColor: c.ground, borderUpColor: c.up, wickUpColor: c.up,
    downColor: c.down, borderDownColor: c.down, wickDownColor: c.down,
  })
  if (props.period === 'intraday') {
    candle.setData([])
    const pts = props.intraday ?? []
    const last = pts[pts.length - 1]?.price
    const pc = props.prevClose ?? pts[0]?.price ?? 0
    const col = last == null || last === pc ? c.ink2 : last > pc ? c.up : c.down
    line.applyOptions({ color: col, visible: true })
    line.setData(pts.map((p) => ({ time: minuteTime(p.minute), value: yuan(p.price) })))
    vol.setData(pts.map((p, i) => ({
      time: minuteTime(p.minute), value: p.volume,
      color: hexA(p.price >= (i ? pts[i - 1].price : pc) ? c.up : c.down, 0.35),
    })))
    if (prevLine) line.removePriceLine(prevLine)
    prevLine = props.prevClose ? line.createPriceLine({ price: yuan(props.prevClose), color: c.base, lineStyle: LineStyle.Dashed, lineWidth: 1, axisLabelVisible: false, title: '' }) : null
  } else {
    line.setData([])
    line.applyOptions({ visible: false })
    if (prevLine) { line.removePriceLine(prevLine); prevLine = null }
    candle.setData(bars.value.map((k) => {
      const base = { time: k.time as Time, open: yuan(k.open), high: yuan(k.high), low: yuan(k.low), close: yuan(k.close) }
      if (!k.replay) return base
      const up = k.close >= k.open
      const col = hexA(up ? c.up : c.down, 0.45)
      return { ...base, color: up ? c.ground : col, borderColor: col, wickColor: col }
    }))
    // a day without trades draws nothing (a zero bar would still leave a 1px tick on the baseline)
    vol.setData(bars.value.map((k) => ({
      time: k.time as Time, value: k.volume,
      color: k.volume ? hexA(k.close >= k.open ? c.up : c.down, k.replay ? 0.18 : 0.35) : 'transparent',
    })))
  }
  ch.timeScale().fitContent()
  place()
  floor()
}

/** Lowest price on screen (币), so the axis does not label the volume band below it (0.00, negative prices). */
function floor() {
  const ch = chart.value
  if (!ch) return
  let lo = Infinity
  if (props.period === 'intraday') {
    for (const p of props.intraday ?? []) lo = Math.min(lo, yuan(p.price))
    if (props.prevClose) lo = Math.min(lo, yuan(props.prevClose))
  } else {
    const r = ch.timeScale().getVisibleLogicalRange()
    const bs = bars.value
    const a = Math.max(0, Math.floor(r?.from ?? 0))
    const b = Math.min(bs.length - 1, Math.ceil(r?.to ?? bs.length - 1))
    for (let i = a; i <= b; i++) lo = Math.min(lo, yuan(bs[i].low))
  }
  // a tick a hair below the low still labels the bottom of the price range
  const f = Number.isFinite(lo) ? lo - 0.005 : -Infinity
  if (f === axisFloor) return
  axisFloor = f
  ch.applyOptions({ localization: { locale: 'zh-CN', priceFormatter: fmtPrice } })
}

/** A pushed trade changed the last minute or added one: extend the line in place (no full redraw). */
let shown = 0
function extend() {
  const ch = chart.value
  const pts = props.intraday ?? []
  if (!ch || !line || !vol || props.period !== 'intraday' || !pts.length) return
  const c = colors()
  const i = pts.length - 1
  const p = pts[i]
  const pc = props.prevClose ?? pts[0].price
  line.applyOptions({ color: p.price === pc ? c.ink2 : p.price > pc ? c.up : c.down })
  line.update({ time: minuteTime(p.minute), value: yuan(p.price) })
  vol.update({ time: minuteTime(p.minute), value: p.volume, color: hexA(p.price >= (i ? pts[i - 1].price : pc) ? c.up : c.down, 0.35) })
  if (shown !== pts.length) ch.timeScale().fitContent()
  shown = pts.length
  laterDot()
}

/** Place the dot once the chart has re-laid out its scales (next frames). */
function laterDot() {
  requestAnimationFrame(() => requestAnimationFrame(placeDot))
}

/** The pulsing dot on the last intraday point. */
function placeDot() {
  const ch = chart.value
  const pts = props.intraday ?? []
  if (!ch || !line || props.period !== 'intraday' || !props.live || !pts.length) {
    dot.value = null
    return
  }
  const p = pts[pts.length - 1]
  const x = ch.timeScale().timeToCoordinate(minuteTime(p.minute))
  const y = line.priceToCoordinate(yuan(p.price))
  const pc = props.prevClose ?? pts[0].price
  dot.value = x == null || y == null ? null : { x: Number(x), y: Number(y), up: p.price === pc ? null : p.price > pc }
}

/** Overlay positions for episode and dividend markers. */
function place() {
  const ch = chart.value
  if (!ch || props.period === 'intraday') {
    marks.value = []
    divs.value = []
    launchX.value = null
    return
  }
  const ts = ch.timeScale()
  const have = new Set(bars.value.map((b) => b.time))
  const x = (day: string): number | null => {
    const b = bucket(day, props.period)
    if (!have.has(b)) return null
    const c = ts.timeToCoordinate(b as Time)
    return c == null ? null : Number(c)
  }
  const w = width.value
  const out: { x: number; label: string; key: string }[] = []
  const eps = props.episodes ?? []
  const labelW = (s: string) => 8 + s.length * (props.wide ? 7 : 6.2)
  let lastRight = -Infinity
  // newest first so the latest episodes keep their labels when crowded
  for (let i = eps.length - 1; i >= 0; i--) {
    const e = eps[i]
    const px = x(e.trading_day)
    if (px == null || px < 0 || px > w - 50) continue
    const label = props.wide ? `第 ${e.episode} 集` : String(e.episode)
    const lw = labelW(label)
    const show = lastRight === -Infinity || px + lw / 2 + 2 < lastRight
    out.push({ x: px, label: show ? label : '', key: e.trading_day })
    if (show) lastRight = px - lw / 2
  }
  marks.value = out
  // launch: halfway between the last replay bar and the next (live) bar
  const bs = bars.value
  const k = bs.findIndex((b, i) => i > 0 && !b.replay && bs[i - 1].replay)
  const xa = k > 0 ? ts.timeToCoordinate(bs[k - 1].time as Time) : null
  const xb = k > 0 ? ts.timeToCoordinate(bs[k].time as Time) : null
  const lx = xa != null && xb != null ? (Number(xa) + Number(xb)) / 2 : null
  launchX.value = lx != null && lx >= 0 && lx <= w - 50 ? lx : null
  divs.value = (props.dividends ?? [])
    .map((d) => ({ x: x(d.trading_day), key: d.trading_day, ps: d.per_share }))
    .filter((d): d is { x: number; key: string; ps: number } => d.x != null && d.x >= 0 && d.x <= w - 50)
}

let ro: ResizeObserver | null = null
onMounted(() => {
  if (!host.value) return
  const ch = createChart(host.value, {
    autoSize: true,
    crosshair: { mode: CrosshairMode.Normal },
    localization: { locale: 'zh-CN', priceFormatter: fmtPrice },
    handleScroll: { vertTouchDrag: false },
    rightPriceScale: { scaleMargins: { top: 0.08, bottom: 0.24 } },
  })
  chart.value = ch
  candle = ch.addCandlestickSeries({ priceLineVisible: false, lastValueVisible: true, priceFormat: { type: 'price', precision: 2, minMove: 0.01 } })
  line = ch.addLineSeries({ lineWidth: 2, priceLineVisible: false, lastValueVisible: true, crosshairMarkerRadius: 3 })
  vol = ch.addHistogramSeries({ priceScaleId: 'vol', priceFormat: { type: 'volume' }, lastValueVisible: false, priceLineVisible: false })
  // volume: its own overlay scale, pinned to the bottom fifth, no labels (the candles keep the top three quarters)
  ch.priceScale('vol').applyOptions({ scaleMargins: { top: 0.8, bottom: 0 }, visible: false })
  ch.timeScale().subscribeVisibleLogicalRangeChange(() => {
    place()
    placeDot()
    floor()
  })
  ch.subscribeCrosshairMove((p) => {
    if (!p.time) return (hover.value = null)
    if (props.period === 'intraday') {
      const pt = (props.intraday ?? []).find((q) => minuteTime(q.minute) === p.time)
      hover.value = pt ? { t: pt.minute.slice(11, 16), c: pt.price, v: pt.volume } : null
    } else {
      const k = bars.value.find((b) => b.time === p.time)
      hover.value = k ? { t: k.time, o: k.open, h: k.high, l: k.low, c: k.close, v: k.volume } : null
    }
  })
  ro = new ResizeObserver(() => {
    width.value = host.value?.clientWidth ?? 0
    requestAnimationFrame(place)
  })
  ro.observe(host.value)
  width.value = host.value.clientWidth
  draw()
})
watch(() => [props.candles, props.period, props.intraday, props.episodes, props.dividends], () => {
  draw()
  shown = props.intraday?.length ?? 0
  laterDot()
})
watch(() => {
  const p = props.intraday
  return p && p.length ? `${p.length}|${p[p.length - 1].price}|${p[p.length - 1].volume}` : ''
}, extend)
watch(() => props.live, laterDot)
watch(tick, draw)
onBeforeUnmount(() => {
  ro?.disconnect()
  chart.value?.remove()
})
</script>

<template>
  <div class="kc" :class="{ wide }">
    <div class="readout num" aria-live="off">
      <template v-if="hover">
        <span>{{ hover.t }}</span>
        <template v-if="hover.o != null"><span>开 {{ money(hover.o) }}</span><span>高 {{ money(hover.h) }}</span><span>低 {{ money(hover.l) }}</span></template>
        <span>收 {{ money(hover.c) }}</span><span>量 {{ qty(hover.v) }}</span>
      </template>
    </div>
    <div class="area">
      <div ref="host" class="chart" role="img" :aria-label="period === 'intraday' ? '分时走势' : 'K 线，标出了各集播出日、分红日和开市日'"></div>
      <span v-if="dot" class="ldot" :class="dot.up == null ? 'flat' : dot.up ? 'up' : 'down'" :style="{ left: dot.x + 'px', top: dot.y + 'px' }" aria-hidden="true"></span>
      <svg class="over" aria-hidden="true">
        <line v-if="launchX != null" :x1="launchX" :x2="launchX" y1="0" y2="100%" class="launch" />
        <line v-for="m in marks" :key="m.key" :x1="m.x" :x2="m.x" y1="6" y2="100%" class="ep" />
      </svg>
    </div>
    <div v-if="marks.length || divs.length" class="strip" aria-hidden="true">
      <span v-for="m in marks.filter((q) => q.label)" :key="m.key" class="eplabel num" :style="{ left: m.x + 'px' }">{{ m.label }}</span>
      <span v-for="d in divs" :key="d.key" class="div" :style="{ left: d.x + 'px' }" :title="`分红每股 ${money(d.ps)}`"></span>
    </div>
  </div>
</template>

<style scoped>
.kc { display: flex; flex-direction: column; }
.readout { display: flex; flex-wrap: wrap; gap: 2px 10px; font-size: 11px; color: var(--ink-2); min-height: 17px; padding: 0 var(--gutter); }
.area { position: relative; }
.chart { width: 100%; height: 230px; }
.wide .chart { height: 330px; }
.over { position: absolute; left: 0; top: 0; width: 100%; height: calc(100% - 26px); pointer-events: none; overflow: visible; }
.launch { stroke: var(--base-line); stroke-width: 1; }
.ep { stroke: var(--accent); stroke-width: 1; stroke-dasharray: 2 3; opacity: 0.55; }
.strip { position: relative; height: 32px; }
.eplabel {
  position: absolute; top: 2px; transform: translateX(-50%); background: var(--accent); color: #fff;
  font-size: 10px; line-height: 15px; padding: 0 4px; min-width: 18px; text-align: center; white-space: nowrap;
}
.wide .eplabel { font-size: 11px; line-height: 17px; font-family: var(--font); }
.ldot { position: absolute; z-index: 2; width: 7px; height: 7px; margin: -3.5px 0 0 -3.5px; border-radius: 50%; background: var(--ink-2); pointer-events: none; transition: left 0.3s ease-out, top 0.3s ease-out; }
.ldot.up { background: var(--up); }
.ldot.down { background: var(--down); }
.ldot::after { content: ''; position: absolute; inset: 0; border-radius: 50%; background: inherit; animation: ldot 1.8s ease-out infinite; }
@keyframes ldot { 0% { transform: scale(1); opacity: 0.5; } 80%, 100% { transform: scale(3); opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .ldot { transition: none; } .ldot::after { animation: none; opacity: 0; } }
.div { position: absolute; top: 22px; width: 7px; height: 7px; background: var(--ink); transform: translateX(-50%) rotate(45deg); }
</style>
