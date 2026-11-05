<script setup lang="ts">
// Weekly revenue stacked by market. Tap/hover a bar to read its numbers.
import { computed, ref } from 'vue'
import type { WeekRow } from '../api'
import { big, shortDate } from '../format'

const props = defineProps<{ weeks: WeekRow[] }>()
const rows = computed(() => props.weeks.slice(-16))
const sel = ref<number | null>(null)
const W = 600, H = 160, L = 40, B = 20, T = 8

const max = computed(() => Math.max(1, ...rows.value.map((r) => r.us + r.cn + r.jp)))
const ticks = computed(() => {
  const m = max.value
  const step = niceStep(m / 3)
  const out: number[] = []
  for (let v = 0; v <= m + 1e-9; v += step) out.push(v)
  return out
})
const top = computed(() => Math.max(max.value, ticks.value[ticks.value.length - 1]))
function niceStep(x: number) {
  const p = Math.pow(10, Math.floor(Math.log10(x || 1)))
  const n = x / p
  return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10) * p
}
const bw = computed(() => (W - L - 4) / Math.max(1, rows.value.length))
const y = (v: number) => T + (H - T - B) * (1 - v / top.value)
const segs = (r: WeekRow) => {
  let acc = 0
  return (['us', 'cn', 'jp'] as const).map((k) => {
    const v = r[k]
    const s = { k, y0: y(acc), y1: y(acc + v), v }
    acc += v
    return s
  })
}
const active = computed(() => (sel.value != null ? rows.value[sel.value] : rows.value[rows.value.length - 1]))
const COL = { us: 'var(--s-us)', cn: 'var(--s-cn)', jp: 'var(--s-jp)' }
</script>

<template>
  <div>
    <div class="legend small">
      <span><i :style="{ background: COL.cn }"></i>中国</span>
      <span><i :style="{ background: COL.us }"></i>欧美</span>
      <span><i :style="{ background: COL.jp }"></i>日本</span>
      <span v-if="active" class="muted readout num">
        {{ shortDate(active.week) }} 起一周：{{ big(active.revenue) }}（美 {{ big(active.us) }} · 中 {{ big(active.cn) }} · 日 {{ big(active.jp) }}）
      </span>
    </div>
    <svg :viewBox="`0 0 ${W} ${H}`" class="bars" role="img" aria-label="每周营收，按市场分">
      <g v-for="t in ticks" :key="t">
        <line :x1="L" :x2="W" :y1="y(t)" :y2="y(t)" stroke="var(--rule)" />
        <text :x="L - 6" :y="y(t) + 4" text-anchor="end" class="axis">{{ big(t) }}</text>
      </g>
      <g v-for="(r, i) in rows" :key="r.week" @mouseenter="sel = i" @click="sel = i" class="col">
        <rect :x="L + i * bw" :y="T" :width="bw" :height="H - T - B" fill="transparent" />
        <template v-for="s in segs(r)" :key="s.k">
          <rect v-if="s.v > 0" :x="L + i * bw + bw * 0.18" :y="s.y1" :width="bw * 0.64" :height="Math.max(0, s.y0 - s.y1 - 1)" :fill="COL[s.k]"
                :opacity="sel == null || sel === i ? 1 : 0.45" />
        </template>
        <text v-if="i % Math.ceil(rows.length / 6) === 0" :x="L + i * bw + bw / 2" :y="H - 5" text-anchor="middle" class="axis">{{ shortDate(r.week) }}</text>
        <title>{{ r.week }}：{{ big(r.revenue) }}</title>
      </g>
    </svg>
  </div>
</template>

<style scoped>
.legend { display: flex; flex-wrap: wrap; gap: 4px 12px; color: var(--ink-2); margin-bottom: 4px; align-items: center; }
.legend i { display: inline-block; width: 9px; height: 9px; border-radius: 2px; margin-right: 4px; vertical-align: -1px; }
.readout { flex-basis: 100%; }
.bars { width: 100%; height: auto; display: block; }
.axis { font-size: 11px; fill: var(--muted); font-family: var(--font-num); }
.col { cursor: pointer; }
</style>
