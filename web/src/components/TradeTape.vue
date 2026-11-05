<script setup lang="ts">
// Latest trades, newest on top. New rows slide in with a short highlight; big trades (well above the
// usual size here) are set in bold with a tint of their direction.
import { computed } from 'vue'
import { hms, money, qty } from '../format'
const props = defineProps<{ trades: { id?: number; price: number; qty: number; ts: string }[]; limit?: number }>()

// a stable key per trade: its id when pushed, else time|price|qty plus the occurrence counted from the
// oldest (REST rows have no id), so a re-fetch does not re-animate rows already shown
function keys(t: { id?: number; price: number; qty: number; ts: string }[]) {
  const seen = new Map<string, number>()
  const out: string[] = new Array(t.length)
  for (let i = t.length - 1; i >= 0; i--) {
    const base = `${t[i].ts}|${t[i].price}|${t[i].qty}`
    const n = (seen.get(base) ?? 0) + 1
    seen.set(base, n)
    out[i] = `${base}|${n}`
  }
  return out
}

// colour by tick: up if above the previous (older) trade
const rows = computed(() => {
  const n = props.limit ?? 8
  const k = keys(props.trades)
  const t = props.trades.slice(0, n + 1)
  const sizes = props.trades.slice(0, 30).map((x) => x.qty).sort((a, b) => a - b)
  const median = sizes.length ? sizes[Math.floor(sizes.length / 2)] : 0
  const bigQty = Math.max(500, median * 4)
  return t.slice(0, n).map((x, i) => ({ ...x, key: k[i], up: !t[i + 1] || x.price >= t[i + 1].price, big: x.qty >= bigQty }))
})
</script>

<template>
  <div class="tape">
    <TransitionGroup name="tr">
      <div v-for="t in rows" :key="t.key" class="tr num" :class="{ big: t.big, bu: t.big && t.up, bd: t.big && !t.up }">
        <span class="tmuted">{{ hms(t.ts) }}</span>
        <span :class="t.up ? 'tup' : 'tdown'">{{ money(t.price) }}</span>
        <span class="r">{{ qty(t.qty) }}</span>
      </div>
    </TransitionGroup>
    <p v-if="!rows.length" class="tmuted none">暂无成交</p>
  </div>
</template>

<style scoped>
.tape { display: flex; flex-direction: column; gap: 2px; overflow: hidden; }
.tr { display: grid; grid-template-columns: 64px minmax(0, 1fr) auto; font-size: 12px; padding: 2px 4px; margin: 0 -4px; }
.r { text-align: right; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.big { font-weight: 700; }
.big .r { color: var(--t-ink); }
.bu { background: var(--t-flash-up); }
.bd { background: var(--t-flash-down); }
.none { font-size: 12px; }
/* a new trade slides in from the top and its row lights up briefly */
.tr-enter-active { transition: transform 0.25s ease-out, opacity 0.25s ease-out; animation: tr-hl 0.9s ease-out; }
.tr-enter-from { transform: translateY(-100%); opacity: 0; }
.tr-move { transition: transform 0.25s ease-out; }
.tr-leave-active { display: none; }
@keyframes tr-hl { from { background-color: var(--t-flash-neutral); } }
@media (prefers-reduced-motion: reduce) {
  .tr-enter-active, .tr-move { transition: none; animation: tr-hl 0.6s steps(1, end); }
}
</style>
