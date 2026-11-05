<script setup lang="ts">
// Daily candles, A-share colours: up = red hollow, down = green solid. Replay days (before launch)
// are drawn muted, and the launch day carries a marker.
import { createChart, CrosshairMode, type IChartApi, type ISeriesApi, type Time } from 'lightweight-charts'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import type { Candle } from '../api'
import { cssVar, useThemeTick } from '../composables/useTheme'
import { money } from '../format'

const props = defineProps<{ candles: Candle[] }>()
const el = ref<HTMLDivElement>()
const hover = ref<Candle | null>(null)
let chart: IChartApi | null = null
let series: ISeriesApi<'Candlestick'> | null = null
let vol: ISeriesApi<'Histogram'> | null = null
const tick = useThemeTick()

function colors() {
  return {
    up: cssVar('--up'), down: cssVar('--down'), muted: cssVar('--muted'), rule: cssVar('--rule'),
    surface: cssVar('--surface'), ink2: cssVar('--ink-2'), seal: cssVar('--seal'),
  }
}

function hexA(hex: string, a: number) {
  const m = hex.replace('#', '')
  if (m.length !== 6) return hex
  const n = parseInt(m, 16)
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`
}

function apply() {
  if (!chart || !series || !vol) return
  const c = colors()
  chart.applyOptions({
    layout: { background: { color: c.surface }, textColor: c.ink2, fontSize: 11, fontFamily: cssVar('--font-num') },
    grid: { vertLines: { visible: false }, horzLines: { color: c.rule } },
    rightPriceScale: { borderVisible: false },
    timeScale: { borderVisible: false },
  })
  series.applyOptions({
    upColor: 'rgba(0,0,0,0)', borderUpColor: c.up, wickUpColor: c.up,
    downColor: c.down, borderDownColor: c.down, wickDownColor: c.down,
  })
  const data = props.candles.map((k) => {
    const up = k.close >= k.open
    if (!k.replay) return { time: k.time as Time, open: k.open, high: k.high, low: k.low, close: k.close }
    const col = hexA(up ? c.up : c.down, 0.45)
    return {
      time: k.time as Time, open: k.open, high: k.high, low: k.low, close: k.close,
      color: up ? 'rgba(0,0,0,0)' : col, borderColor: col, wickColor: col,
    }
  })
  series.setData(data)
  vol.setData(props.candles.map((k) => ({
    time: k.time as Time, value: k.volume,
    color: hexA(k.close >= k.open ? c.up : c.down, k.replay ? 0.2 : 0.45),
  })))
  const firstLive = props.candles.find((k) => !k.replay)
  series.setMarkers(firstLive && props.candles[0]?.replay
    ? [{ time: firstLive.time as Time, position: 'belowBar', color: c.seal, shape: 'arrowUp', text: '开市' }]
    : [])
}

onMounted(() => {
  if (!el.value) return
  chart = createChart(el.value, {
    autoSize: true,
    crosshair: { mode: CrosshairMode.Normal },
    localization: { locale: 'zh-CN', priceFormatter: (p: number) => p.toFixed(2) },
    handleScroll: { vertTouchDrag: false },
  })
  series = chart.addCandlestickSeries({ priceLineVisible: true, priceFormat: { type: 'price', precision: 2, minMove: 0.01 } })
  vol = chart.addHistogramSeries({ priceScaleId: 'vol', priceFormat: { type: 'volume' }, lastValueVisible: false, priceLineVisible: false })
  chart.priceScale('vol').applyOptions({ scaleMargins: { top: 0.82, bottom: 0 } })
  series.priceScale().applyOptions({ scaleMargins: { top: 0.08, bottom: 0.22 } })
  chart.subscribeCrosshairMove((p) => {
    hover.value = p.time ? props.candles.find((k) => k.time === p.time) ?? null : null
  })
  apply()
  chart.timeScale().fitContent()
})

watch(() => props.candles, () => apply())
watch(tick, () => apply())
onBeforeUnmount(() => chart?.remove())
</script>

<template>
  <div class="wrap">
    <div class="readout num" aria-live="off">
      <template v-if="hover">
        <span>{{ hover.time }}</span>
        <span>开 {{ money(hover.open) }}</span><span>高 {{ money(hover.high) }}</span>
        <span>低 {{ money(hover.low) }}</span><span>收 {{ money(hover.close) }}</span>
        <span v-if="hover.replay" class="muted">回放</span>
        <span v-else>量 {{ hover.volume }}</span>
      </template>
      <span v-else class="muted hint">淡色 K 线为开市前的数据回放，由真实热度推算；实心红绿为开市后的真实成交</span>
    </div>
    <div ref="el" class="chart" role="img" aria-label="日 K 线"></div>
  </div>
</template>

<style scoped>
.wrap { display: flex; flex-direction: column; gap: 6px; }
.readout { display: flex; flex-wrap: wrap; gap: 4px 10px; font-size: 11px; color: var(--ink-2); min-height: 18px; }
.hint { font-family: var(--font); }
.chart { width: 100%; height: 280px; }
@media (min-width: 900px) { .chart { height: 340px; } }
</style>
