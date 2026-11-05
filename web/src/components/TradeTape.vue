<script setup lang="ts">
import { computed } from 'vue'
import { hms, money, qty } from '../format'
const props = defineProps<{ trades: { price: number; qty: number; ts: string }[]; limit?: number }>()
// colour by tick: up if above the previous (older) trade
const rows = computed(() => {
  const t = props.trades.slice(0, (props.limit ?? 8) + 1)
  return t.slice(0, props.limit ?? 8).map((x, i) => ({ ...x, up: !t[i + 1] || x.price >= t[i + 1].price }))
})
</script>

<template>
  <div class="tape">
    <div v-for="(t, i) in rows" :key="i" class="tr num">
      <span class="tmuted">{{ hms(t.ts) }}</span>
      <span :class="t.up ? 'tup' : 'tdown'">{{ money(t.price) }}</span>
      <span class="r">{{ qty(t.qty) }}</span>
    </div>
    <p v-if="!rows.length" class="tmuted none">暂无成交</p>
  </div>
</template>

<style scoped>
.tape { display: flex; flex-direction: column; gap: 2px; }
.tr { display: grid; grid-template-columns: 64px minmax(0, 1fr) auto; font-size: 12px; padding: 2px 0; }
.r { text-align: right; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.none { font-size: 12px; }
</style>
