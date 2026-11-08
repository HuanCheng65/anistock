<script setup lang="ts">
// The contract at a glance (docs/design/futures-ui.md §11): one bar per day of the period, solid once its index is
// published, an empty dashed box until then; the market's expectation of the period average (the price) as a
// line across the whole period, and the average so far as a dotted line over the published days. Settlement
// is the average of all the bars, so the picture is the whole rule. To the left, lighter, the work's heat in the
// days before the period (`before`), what a player has to go on before the period begins. The expectation's path:
// each day's settlement price (`settles`) as a line, then the price now flat to the period's end, so the line
// shows the market revising toward the answer as the days come out.
import { computed } from 'vue'
import type { FuturesContract, HeatDay } from '../../api'
import { addDays, num } from '../../format'

const props = defineProps<{
  c: FuturesContract
  index: HeatDay[]
  /** the work's index on the days before the period (oldest first) */
  before?: { day: string; index: number }[]
  /** each closed day's settlement price, in hundredths of a point (oldest first) */
  settles?: { day: string; settle: number }[]
  today?: string
  wide?: boolean
}>()

const W = computed(() => (props.wide ? 900 : 358))
const H = 200
const top = 16
const base = 178
const days = computed(() => Array.from({ length: props.c.days_total }, (_, i) => addDays(props.c.first_day, i)))
const byDay = computed(() => new Map(props.index.map((d) => [d.day, d.index])))
const price = computed(() => props.c.last_price / 100)
const avg = computed(() => props.c.average_so_far)
const prior = computed(() => props.before ?? [])
const path = computed(() => (props.settles ?? []).map((s) => ({ day: s.day, v: s.settle / 100 })))
const max = computed(() =>
  Math.max(price.value, ...props.index.map((d) => d.index), ...prior.value.map((d) => d.index), ...path.value.map((p) => p.v), 1) * 1.18)
const y = (v: number) => base - (v / max.value) * (base - top)
const GAP = 10
const slot = computed(() => (W.value - 52 - (prior.value.length ? GAP : 0)) / (days.value.length + prior.value.length))
/** where the period starts */
const o = computed(() => (prior.value.length ? prior.value.length * slot.value + GAP : 0))
const x = (i: number) => o.value + i * slot.value
const px = (i: number) => i * slot.value
const bw = computed(() => Math.max(2, slot.value - (props.wide ? 4 : 3)))
const known = computed(() => days.value.filter((d) => byDay.value.has(d)).length)
/** the middle of a day's slot, in the period or among the days before it (null: off the chart) */
function xOf(day: string): number | null {
  const i = days.value.indexOf(day)
  if (i >= 0) return x(i) + bw.value / 2
  const j = prior.value.findIndex((d) => d.day === day)
  return j >= 0 ? px(j) + bw.value / 2 : null
}
const pts = computed(() => path.value.map((p) => ({ x: xOf(p.day), y: y(p.v) })).filter((p): p is { x: number; y: number } => p.x != null))
/** where the flat line of the price now begins: after the last settlement drawn, else the period's start */
const nowX = computed(() => (pts.value.length ? pts.value[pts.value.length - 1].x : o.value))
const left = computed(() => days.value.length - known.value)
/** where today falls: after the last published day */
const cut = computed(() => x(known.value) - (slot.value - bw.value) / 2)
const label = (d: string) => `${Number(d.slice(5, 7))}/${Number(d.slice(8))}`
const aria = computed(() => {
  const parts = [`已公布 ${known.value} 天的每日热度`]
  if (avg.value != null) parts.push(`平均 ${num(avg.value)}`)
  parts.push(`市场预期 ${props.c.days_total} 天平均 ${num(price.value)}`)
  if (left.value) parts.push(`还有 ${left.value} 天未公布`)
  return parts.join('；')
})
</script>

<template>
  <svg class="fc" role="img" :aria-label="aria" :viewBox="`0 0 ${W} ${H}`" :width="W" :height="H">
    <g class="pre">
      <rect v-for="(d, i) in prior" :key="d.day" :x="px(i)" :y="y(d.index)" :width="bw" :height="base - y(d.index)" />
    </g>
    <template v-if="prior.length">
      <line class="start" :x1="o - GAP / 2" y1="8" :x2="o - GAP / 2" :y2="base + 4" />
      <text class="t" :x="o" y="18">合约期</text>
      <text class="t num muted" x="0" :y="H - 4">{{ label(prior[0].day) }}</text>
    </template>
    <g class="pub">
      <template v-for="(d, i) in days" :key="d">
        <rect v-if="byDay.has(d)" :x="x(i)" :y="y(byDay.get(d)!)" :width="bw" :height="base - y(byDay.get(d)!)" />
        <rect v-else class="todo" :x="x(i) + 0.5" :y="base - 28" :width="bw - 1" height="28" />
      </template>
    </g>
    <text v-if="left && x(days.length) - cut > 100" class="t muted" :x="(cut + x(days.length)) / 2" :y="base - 36" text-anchor="middle">还有 {{ left }} 天未公布</text>
    <line v-if="avg != null && known" class="avg" :x1="o" :y1="y(avg)" :x2="cut" :y2="y(avg)" />
    <polyline v-if="pts.length > 1" class="mkt path" :points="[...pts.map((p) => `${p.x},${p.y}`), `${nowX},${y(price)}`].join(' ')" />
    <line class="mkt" :x1="nowX" :y1="y(price)" :x2="x(days.length)" :y2="y(price)" />
    <text class="t up" :x="x(days.length) + 4" :y="y(price) - 3">市场预期</text>
    <text class="t up num" :x="x(days.length) + 4" :y="y(price) + 11">{{ num(price) }}</text>
    <template v-if="known && left && (!prior.length || known >= 5)">
      <line class="now" :x1="cut" y1="8" :x2="cut" :y2="base + 4" />
      <text class="t" :x="cut - 4" y="18" text-anchor="end">今天</text>
    </template>
    <text class="t num muted" :x="o" :y="H - 4" :text-anchor="prior.length ? 'middle' : 'start'">{{ label(c.first_day) }}</text>
    <text class="t num muted" :x="x(days.length)" :y="H - 4" text-anchor="end">{{ label(c.last_day) }}</text>
  </svg>
</template>

<style scoped>
.fc { display: block; max-width: 100%; height: auto; }
.pub rect { fill: var(--ink); }
.pre rect { fill: var(--muted); opacity: 0.45; }
.start { stroke: var(--rule); }
.pub rect.todo { fill: none; stroke: var(--hair); stroke-dasharray: 2 2; }
.avg { stroke: var(--ink); stroke-width: 1.5; stroke-dasharray: 1 3; }
.mkt { stroke: var(--up); stroke-width: 2; fill: none; }
.mkt.path { stroke-width: 1.5; stroke-linejoin: round; }
.now { stroke: var(--hair); }
.t { font-size: 11px; fill: var(--ink-2); font-family: var(--font); }
.t.num { font-family: var(--font-num); }
.t.muted { fill: var(--muted); }
.t.up { fill: var(--up); }
</style>
