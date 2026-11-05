<script setup lang="ts">
// Five levels each side, on the terminal surface. "split": bids | asks side by side (phone);
// "stack": asks above the last price, bids below (desktop). A price fills the order form.
import { computed } from 'vue'
import type { BookLevel } from '../api'
import { money, qty } from '../format'

const props = defineProps<{ bids: BookLevel[]; asks: BookLevel[]; layout: 'split' | 'stack'; last?: number; prevClose?: number }>()
const emit = defineEmits<{ pick: [price: number] }>()
const max = computed(() => Math.max(1, ...props.bids.map((b) => b.qty), ...props.asks.map((a) => a.qty)))
const bids = computed(() => props.bids.slice(0, 5))
const asks = computed(() => props.asks.slice(0, 5))
const asksDesc = computed(() => [...asks.value].reverse())
const spread = computed(() => (props.bids[0] && props.asks[0] ? props.asks[0].price - props.bids[0].price : null))
const lastCls = computed(() => (props.last == null || props.prevClose == null ? '' : props.last > props.prevClose ? 'tup' : props.last < props.prevClose ? 'tdown' : ''))
const w = (q: number) => Math.round((q / max.value) * 100) + '%'
</script>

<template>
  <div class="book" :class="layout">
    <template v-if="layout === 'split'">
      <div class="col">
        <button v-for="b in bids" :key="'b' + b.price" type="button" class="lv num" @click="emit('pick', b.price)">
          <span class="bar bid" :style="{ width: w(b.qty) }"></span>
          <span class="p tup">{{ money(b.price) }}</span><span v-flash.neutral="b.qty" class="q">{{ qty(b.qty) }}</span>
        </button>
      </div>
      <div class="col">
        <button v-for="a in asks" :key="'a' + a.price" type="button" class="lv num" @click="emit('pick', a.price)">
          <span class="bar ask left" :style="{ width: w(a.qty) }"></span>
          <span class="p tdown">{{ money(a.price) }}</span><span v-flash.neutral="a.qty" class="q">{{ qty(a.qty) }}</span>
        </button>
      </div>
    </template>
    <template v-else>
      <button v-for="a in asksDesc" :key="'a' + a.price" type="button" class="lv num" @click="emit('pick', a.price)">
        <span class="bar ask" :style="{ width: w(a.qty) }"></span>
        <span class="p tdown">{{ money(a.price) }}</span><span v-flash.neutral="a.qty" class="q">{{ qty(a.qty) }}</span>
      </button>
      <div class="mid num">
        <span v-flash="last" :class="lastCls">{{ money(last) }}</span>
        <span class="tmuted sp">{{ spread != null ? money(spread) : '' }}</span>
      </div>
      <button v-for="b in bids" :key="'b' + b.price" type="button" class="lv num" @click="emit('pick', b.price)">
        <span class="bar bid" :style="{ width: w(b.qty) }"></span>
        <span class="p tup">{{ money(b.price) }}</span><span v-flash.neutral="b.qty" class="q">{{ qty(b.qty) }}</span>
      </button>
    </template>
    <p v-if="!bids.length && !asks.length" class="none tmuted">暂无委托</p>
  </div>
</template>

<style scoped>
.book.split { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.col { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.stack { display: flex; flex-direction: column; gap: 2px; }
.lv {
  position: relative; display: flex; justify-content: space-between; gap: 8px; font-size: 12px; padding: 2px 4px;
  border: 0; background: transparent; color: var(--t-ink); text-align: left; width: 100%;
}
.bar { position: absolute; right: 0; top: 0; bottom: 0; opacity: 0.16; pointer-events: none; transition: width 0.3s ease-out; }
@media (prefers-reduced-motion: reduce) { .bar { transition: none; } }
.bar.left { right: auto; left: 0; }
.bar.bid { background: var(--t-up); }
.bar.ask { background: var(--t-down); }
.p, .q { position: relative; }
.q { padding: 0 2px; margin: 0 -2px; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.mid { display: flex; justify-content: space-between; align-items: baseline; padding: 6px 4px; font-size: 15px; border-top: 1px solid var(--t-line); border-bottom: 1px solid var(--t-line); margin: 4px 0; }
.sp { font-size: 12px; }
.none { grid-column: 1 / -1; font-size: 12px; padding: 4px; }
</style>
