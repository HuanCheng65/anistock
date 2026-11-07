<script setup lang="ts">
// The contract at a glance (docs/v3-futures-ui.md §11): one bar per day of the period, solid once its index is
// published, an empty dashed box until then; the market's expectation of the period average (the price) as a
// line across the whole period, and the average so far as a dotted line over the published days. Settlement
// is the average of all the bars, so the picture is the whole rule.
import { computed } from 'vue'
import type { FuturesContract, HeatDay } from '../../api'
import { addDays, num } from '../../format'

const props = defineProps<{ c: FuturesContract; index: HeatDay[]; today?: string; wide?: boolean }>()

const W = computed(() => (props.wide ? 900 : 358))
const H = 200
const top = 16
const base = 178
const days = computed(() => Array.from({ length: props.c.days_total }, (_, i) => addDays(props.c.first_day, i)))
const byDay = computed(() => new Map(props.index.map((d) => [d.day, d.index])))
const price = computed(() => props.c.last_price / 100)
const avg = computed(() => props.c.average_so_far)
const max = computed(() => Math.max(price.value, ...props.index.map((d) => d.index), 1) * 1.18)
const y = (v: number) => base - (v / max.value) * (base - top)
const slot = computed(() => (W.value - 52) / days.value.length)
const x = (i: number) => i * slot.value
const bw = computed(() => Math.max(2, slot.value - (props.wide ? 4 : 3)))
const known = computed(() => days.value.filter((d) => byDay.value.has(d)).length)
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
    <g class="pub">
      <template v-for="(d, i) in days" :key="d">
        <rect v-if="byDay.has(d)" :x="x(i)" :y="y(byDay.get(d)!)" :width="bw" :height="base - y(byDay.get(d)!)" />
        <rect v-else class="todo" :x="x(i) + 0.5" :y="base - 28" :width="bw - 1" height="28" />
      </template>
    </g>
    <text v-if="left" class="t muted" :x="(cut + x(days.length)) / 2" :y="base - 36" text-anchor="middle">还有 {{ left }} 天未公布</text>
    <line v-if="avg != null && known" class="avg" x1="0" :y1="y(avg)" :x2="cut" :y2="y(avg)" />
    <line class="mkt" x1="0" :y1="y(price)" :x2="x(days.length)" :y2="y(price)" />
    <text class="t up" :x="x(days.length) + 4" :y="y(price) - 3">市场预期</text>
    <text class="t up num" :x="x(days.length) + 4" :y="y(price) + 11">{{ num(price) }}</text>
    <template v-if="known && left">
      <line class="now" :x1="cut" y1="8" :x2="cut" :y2="base + 4" />
      <text class="t" :x="cut - 4" y="18" text-anchor="end">今天</text>
    </template>
    <text class="t num muted" x="0" :y="H - 4">{{ label(c.first_day) }}</text>
    <text class="t num muted" :x="x(days.length)" :y="H - 4" text-anchor="end">{{ label(c.last_day) }}</text>
  </svg>
</template>

<style scoped>
.fc { display: block; max-width: 100%; height: auto; }
.pub rect { fill: var(--ink); }
.pub rect.todo { fill: none; stroke: var(--hair); stroke-dasharray: 2 2; }
.avg { stroke: var(--ink); stroke-width: 1.5; stroke-dasharray: 1 3; }
.mkt { stroke: var(--up); stroke-width: 2; }
.now { stroke: var(--hair); }
.t { font-size: 11px; fill: var(--ink-2); font-family: var(--font); }
.t.num { font-family: var(--font-num); }
.t.muted { fill: var(--muted); }
.t.up { fill: var(--up); }
</style>
