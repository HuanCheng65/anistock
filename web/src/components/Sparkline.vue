<script setup lang="ts">
import { computed } from 'vue'
const props = withDefaults(defineProps<{ values: number[]; base?: number | null; w?: number; h?: number; stroke?: string }>(), { w: 132, h: 56 })
const geo = computed(() => {
  const v = props.values.filter((x) => Number.isFinite(x))
  if (v.length < 2) return null
  const base = props.base ?? v[0]
  const lo = Math.min(...v, base), hi = Math.max(...v, base)
  const span = hi - lo || 1
  const y = (x: number) => props.h - 4 - ((x - lo) / span) * (props.h - 8)
  const path = v.map((x, i) => (i ? 'L' : 'M') + ((i * props.w) / (v.length - 1)).toFixed(1) + ' ' + y(x).toFixed(1)).join(' ')
  const last = v[v.length - 1]
  return { path, base: y(base).toFixed(1), dir: last > base ? 'up' : last < base ? 'down' : 'flat' }
})
</script>

<template>
  <svg v-if="geo" :width="w" :height="h" :viewBox="`0 0 ${w} ${h}`" aria-hidden="true" class="spark">
    <line x1="0" :y1="geo.base" :x2="w" :y2="geo.base" class="base" />
    <path :d="geo.path" :class="geo.dir" :style="stroke ? { stroke } : undefined" />
  </svg>
</template>

<style scoped>
.spark { flex: none; display: block; overflow: visible; }
.base { stroke: var(--base-line); stroke-width: 1; stroke-dasharray: 2 3; }
path { fill: none; stroke-width: 1.6; stroke-linejoin: round; stroke: var(--ink-2); }
path.up { stroke: var(--up); }
path.down { stroke: var(--down); }
</style>
