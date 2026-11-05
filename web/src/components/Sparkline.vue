<script setup lang="ts">
import { computed } from 'vue'

const props = defineProps<{ values: number[]; ref?: number; width?: number; height?: number }>()
const W = computed(() => props.width ?? 64)
const H = computed(() => props.height ?? 24)

const geo = computed(() => {
  const v = props.values.filter((x) => x != null)
  if (v.length < 2) return null
  const lo = Math.min(...v), hi = Math.max(...v)
  const span = hi - lo || hi * 0.01 || 1
  const x = (i: number) => (i / (v.length - 1)) * (W.value - 2) + 1
  const y = (p: number) => H.value - 2 - ((p - lo) / span) * (H.value - 4)
  const d = v.map((p, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(p).toFixed(1)}`).join('')
  const up = v[v.length - 1] >= (props.ref ?? v[0])
  return { d, up, ex: x(v.length - 1), ey: y(v[v.length - 1]) }
})
</script>

<template>
  <svg :width="W" :height="H" :viewBox="`0 0 ${W} ${H}`" aria-hidden="true" class="spark">
    <template v-if="geo">
      <path :d="geo.d" fill="none" :stroke="geo.up ? 'var(--up)' : 'var(--down)'" stroke-width="1.4" stroke-linejoin="round" />
      <circle :cx="geo.ex" :cy="geo.ey" r="1.8" :fill="geo.up ? 'var(--up)' : 'var(--down)'" />
    </template>
    <line v-else x1="1" :y1="H / 2" :x2="W - 1" :y2="H / 2" stroke="var(--rule-strong)" stroke-dasharray="2 3" />
  </svg>
</template>

<style scoped>
.spark { display: block; }
</style>
