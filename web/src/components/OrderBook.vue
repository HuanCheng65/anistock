<script setup lang="ts">
import { computed } from 'vue'
import type { BookLevel } from '../api'
import { money } from '../format'

const props = defineProps<{ bids: BookLevel[]; asks: BookLevel[]; prevClose: number }>()
const emit = defineEmits<{ pick: [price: number] }>()

const asks = computed(() => {
  const a = props.asks.slice(0, 5)
  return Array.from({ length: 5 }, (_, i) => a[4 - i] ?? null).map((lv, i) => ({ lv, label: `卖${5 - i}` }))
})
const bids = computed(() => {
  const b = props.bids.slice(0, 5)
  return Array.from({ length: 5 }, (_, i) => ({ lv: b[i] ?? null, label: `买${i + 1}` }))
})
const maxQty = computed(() => Math.max(1, ...props.bids.map((b) => b.qty), ...props.asks.map((a) => a.qty)))
const cls = (p: number) => (p > props.prevClose ? 'up' : p < props.prevClose ? 'down' : 'flat')
const empty = computed(() => !props.bids.length && !props.asks.length)
</script>

<template>
  <div class="book">
    <div v-for="row in asks" :key="row.label" class="lvl" :class="{ clickable: row.lv }" @click="row.lv && emit('pick', row.lv.price)">
      <span class="lab">{{ row.label }}</span>
      <span class="px num" :class="row.lv ? cls(row.lv.price) : ''">{{ row.lv ? money(row.lv.price) : '–' }}</span>
      <span class="qty num">{{ row.lv ? row.lv.qty : '' }}</span>
      <span class="bar ask" :style="{ width: row.lv ? (row.lv.qty / maxQty) * 100 + '%' : 0 }"></span>
    </div>
    <div class="mid"></div>
    <div v-for="row in bids" :key="row.label" class="lvl" :class="{ clickable: row.lv }" @click="row.lv && emit('pick', row.lv.price)">
      <span class="lab">{{ row.label }}</span>
      <span class="px num" :class="row.lv ? cls(row.lv.price) : ''">{{ row.lv ? money(row.lv.price) : '–' }}</span>
      <span class="qty num">{{ row.lv ? row.lv.qty : '' }}</span>
      <span class="bar bid" :style="{ width: row.lv ? (row.lv.qty / maxQty) * 100 + '%' : 0 }"></span>
    </div>
    <p v-if="empty" class="small muted note">盘口暂时是空的：开市后做市商约 20 秒内挂出报价。集合竞价时段的挂单要到开盘才撮合。</p>
    <p v-else class="small muted note">点价格可以填入下单框</p>
  </div>
</template>

<style scoped>
.book { font-size: 13px; }
.lvl { position: relative; display: grid; grid-template-columns: 3em 1fr auto; gap: 8px; padding: 3px 4px; align-items: center; }
.lvl.clickable { cursor: pointer; }
.lvl.clickable:hover { background: var(--surface-2); }
.lab { color: var(--muted); font-size: 12px; }
.px { text-align: right; }
.qty { text-align: right; min-width: 4em; color: var(--ink-2); }
.bar { position: absolute; right: 0; top: 2px; bottom: 2px; opacity: 0.12; pointer-events: none; }
.bar.ask { background: var(--down); }
.bar.bid { background: var(--up); }
.mid { border-top: 1px dashed var(--rule-strong); margin: 4px 0; }
.note { margin-top: 6px; }
</style>
