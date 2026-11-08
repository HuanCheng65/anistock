<script setup lang="ts">
// Five levels each side, on the terminal surface. "split": bids | asks side by side (phone);
// "stack": asks above the last price, bids below (desktop). A price fills the order form.
// Each row reads 档位 (买1–买5, 卖1–卖5), price, shares. Prices are not coloured: red and green here mean the side
// (the depth bars), never up or down against the previous close.
import { computed } from 'vue'
import type { BookLevel } from '../api'
import { money, pct, qty } from '../format'

const props = defineProps<{ bids: BookLevel[]; asks: BookLevel[]; layout: 'split' | 'stack'; last?: number; prevClose?: number }>()
const emit = defineEmits<{ pick: [price: number] }>()
const max = computed(() => Math.max(1, ...props.bids.map((b) => b.quantity), ...props.asks.map((a) => a.quantity)))
const bids = computed(() => props.bids.slice(0, 5))
const asks = computed(() => props.asks.slice(0, 5))
const asksDesc = computed(() => [...asks.value].reverse())
const change = computed(() => (props.last == null || !props.prevClose ? null : props.last / props.prevClose - 1))
const lastCls = computed(() => (props.last == null || props.prevClose == null ? '' : props.last > props.prevClose ? 'tup' : props.last < props.prevClose ? 'tdown' : ''))
const w = (q: number) => Math.round((q / max.value) * 100) + '%'
</script>

<template>
  <div class="book" :class="layout">
    <template v-if="layout === 'split'">
      <div class="col">
        <button v-for="(b, i) in bids" :key="'b' + b.price" type="button" class="lv num" :data-guide="i === 0 ? 'bid1' : undefined" @click="emit('pick', b.price)">
          <span class="bar bid" :style="{ width: w(b.quantity) }"></span>
          <span class="k">买{{ i + 1 }}</span><span class="p">{{ money(b.price) }}</span><span v-flash.neutral="b.quantity" class="q">{{ qty(b.quantity) }}</span>
        </button>
      </div>
      <div class="col">
        <button v-for="(a, i) in asks" :key="'a' + a.price" type="button" class="lv num" :data-guide="i === 0 ? 'ask1' : undefined" @click="emit('pick', a.price)">
          <span class="bar ask left" :style="{ width: w(a.quantity) }"></span>
          <span class="k">卖{{ i + 1 }}</span><span class="p">{{ money(a.price) }}</span><span v-flash.neutral="a.quantity" class="q">{{ qty(a.quantity) }}</span>
        </button>
      </div>
    </template>
    <template v-else>
      <button v-for="(a, i) in asksDesc" :key="'a' + a.price" type="button" class="lv num" :data-guide="i === asksDesc.length - 1 ? 'ask1' : undefined" @click="emit('pick', a.price)">
        <span class="bar ask" :style="{ width: w(a.quantity) }"></span>
        <span class="k">卖{{ asksDesc.length - i }}</span><span class="p">{{ money(a.price) }}</span><span v-flash.neutral="a.quantity" class="q">{{ qty(a.quantity) }}</span>
      </button>
      <div class="mid num">
        <span><span class="mk">最新</span><span v-flash="last" :class="lastCls">{{ money(last) }}</span></span>
        <span v-if="change != null" class="chg" :class="lastCls">{{ pct(change) }}</span>
      </div>
      <button v-for="(b, i) in bids" :key="'b' + b.price" type="button" class="lv num" :data-guide="i === 0 ? 'bid1' : undefined" @click="emit('pick', b.price)">
        <span class="bar bid" :style="{ width: w(b.quantity) }"></span>
        <span class="k">买{{ i + 1 }}</span><span class="p">{{ money(b.price) }}</span><span v-flash.neutral="b.quantity" class="q">{{ qty(b.quantity) }}</span>
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
  position: relative; display: flex; align-items: baseline; gap: 8px; font-size: 12px; padding: 2px 4px;
  border: 0; background: transparent; color: var(--t-ink); text-align: left; width: 100%;
}
.bar { position: absolute; right: 0; top: 0; bottom: 0; opacity: 0.16; pointer-events: none; transition: width 0.3s ease-out; }
@media (prefers-reduced-motion: reduce) { .bar { transition: none; } }
.bar.left { right: auto; left: 0; }
.bar.bid { background: var(--t-up); }
.bar.ask { background: var(--t-down); }
.p, .q, .k { position: relative; }
.k { flex: none; width: 2.2em; font-family: var(--font); color: var(--t-ink-2); }
.p { flex: 1; min-width: 0; }
.q { color: var(--t-ink-3); }
.q { padding: 0 2px; margin: 0 -2px; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.mid { display: flex; justify-content: space-between; align-items: baseline; padding: 6px 4px; font-size: 15px; border-top: 1px solid var(--t-line); border-bottom: 1px solid var(--t-line); margin: 4px 0; }
.mk { font-family: var(--font); font-size: 12px; color: var(--t-ink-2); margin-right: 8px; }
.chg { font-size: 12px; }
.none { grid-column: 1 / -1; font-size: 12px; padding: 4px; }
</style>
