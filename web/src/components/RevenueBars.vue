<script setup lang="ts">
// Weekly revenue, stacked by market (us bottom, cn, jp top), with the market split of the period. Pointing at
// (or focusing, or tapping) a week shows its revenue, per-share profit and dividend in the readout; without a
// choice the readout shows the latest week. ←/→ move between weeks.
import { computed, ref, watch } from 'vue'
import type { RevenueWeek } from '../api'
import { addDays, bigMoney, money, share } from '../format'
const props = withDefaults(defineProps<{ weeks: RevenueWeek[]; n?: number; h?: number; today?: string }>(), { n: 8, h: 96 })
const shown = computed(() => props.weeks.slice(-props.n))
const max = computed(() => Math.max(1, ...shown.value.map((w) => w.by_market.us + w.by_market.cn + w.by_market.jp)))
const split = computed(() => {
  const t = shown.value.reduce((a, w) => ({ us: a.us + w.by_market.us, cn: a.cn + w.by_market.cn, jp: a.jp + w.by_market.jp }), { us: 0, cn: 0, jp: 0 })
  const s = t.us + t.cn + t.jp || 1
  return { us: t.us / s, cn: t.cn / s, jp: t.jp / s }
})
const px = (v: number) => Math.max(0, (v / max.value) * (props.h - 6)) + 'px'

/** the chosen week (index into `shown`), null: the latest */
const picked = ref<number | null>(null)
const hovering = ref(false)
watch(() => props.weeks.length, () => (picked.value = null))
const cur = computed(() => picked.value ?? shown.value.length - 1)
const w = computed(() => shown.value[cur.value] ?? null)

function label(x: RevenueWeek): string {
  const end = addDays(x.week_start, 6)
  if (props.today && props.today >= x.week_start && props.today <= end) return '本周'
  return `${x.week_start.slice(5)} 起`
}
function aria(x: RevenueWeek): string {
  return `${label(x)}一周，营收 ${bigMoney(x.revenue)}，每股收益 ${money(x.earnings_per_share)}，每股分红 ${money(x.dividend_per_share)}`
}

const bars = ref<HTMLElement | null>(null)
function focusAt(i: number) {
  picked.value = i
  bars.value?.querySelectorAll<HTMLElement>('.col')[i]?.focus()
}
function onKey(e: KeyboardEvent) {
  const n = shown.value.length
  let j = -1
  if (e.key === 'ArrowRight') j = Math.min(n - 1, cur.value + 1)
  else if (e.key === 'ArrowLeft') j = Math.max(0, cur.value - 1)
  else if (e.key === 'Home') j = 0
  else if (e.key === 'End') j = n - 1
  if (j < 0) return
  e.preventDefault()
  focusAt(j)
}
function leave() {
  hovering.value = false
  // a pointer leaving gives the readout back to the latest week; a tap or keyboard choice stays
  if (!bars.value?.contains(document.activeElement)) picked.value = null
}
</script>

<template>
  <div class="rev">
    <div class="read" aria-live="polite">
      <span class="wk">{{ w ? label(w) : '' }}</span>
      <dl v-if="w">
        <div><dt>营收</dt><dd class="num">{{ bigMoney(w.revenue) }}</dd></div>
        <div><dt>每股收益</dt><dd class="num">{{ money(w.earnings_per_share) }}</dd></div>
        <div><dt>每股分红</dt><dd class="num">{{ money(w.dividend_per_share) }}</dd></div>
      </dl>
    </div>
    <div ref="bars" class="bars" :class="{ dim: picked != null || hovering }" :style="{ height: h + 'px' }" role="group" aria-label="每周营收"
         @pointerleave="leave" @keydown="onKey">
      <button v-for="(x, i) in shown" :key="x.week_start" type="button" class="col" :class="{ on: i === cur }"
              :tabindex="i === cur ? 0 : -1" :aria-label="aria(x)" :aria-pressed="i === cur"
              @pointerenter="hovering = true; picked = i" @focus="picked = i" @click="picked = i">
        <span class="jp" :style="{ height: px(x.by_market.jp) }"></span>
        <span class="cn" :style="{ height: px(x.by_market.cn) }"></span>
        <span class="us" :style="{ height: px(x.by_market.us) }"></span>
      </button>
    </div>
    <div class="legend">
      <span><i class="us"></i>欧美 <b class="num">{{ share(split.us) }}</b></span>
      <span><i class="cn"></i>中国 <b class="num">{{ share(split.cn) }}</b></span>
      <span><i class="jp"></i>日本 <b class="num">{{ share(split.jp) }}</b></span>
    </div>
  </div>
</template>

<style scoped>
.rev { display: flex; flex-direction: column; gap: 10px; }
.read { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; min-height: 36px; }
.wk { font-size: 12px; color: var(--muted); white-space: nowrap; }
.read dl { display: flex; gap: 18px; }
.read dl div { display: flex; flex-direction: column; align-items: flex-end; gap: 1px; }
.read dt { font-size: 11px; color: var(--muted); white-space: nowrap; }
.read dd { font-size: 14px; color: var(--ink); }
.bars { display: flex; align-items: flex-end; gap: 6px; touch-action: pan-y; }
.col {
  flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; justify-content: flex-end; height: 100%;
  border: 0; padding: 0; background: transparent; cursor: pointer; transition: opacity var(--dur-1) var(--ease-out);
}
.col:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
.dim .col:not(.on) { opacity: 0.4; }
.col span { display: block; transition: height var(--dur-2) var(--ease-out); }
.us { background: var(--mk-us); }
.cn { background: var(--mk-cn); }
.jp { background: var(--mk-jp); }
.legend { display: flex; gap: 16px; font-size: 12px; color: var(--ink-2); flex-wrap: wrap; }
.legend span { display: flex; align-items: center; gap: 6px; }
.legend i { width: 10px; height: 10px; display: inline-block; }
.legend b { font-weight: 400; color: var(--ink); }
@media (prefers-reduced-motion: reduce) { .col, .col span { transition: none; } }
</style>
