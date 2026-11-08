<script setup lang="ts">
// 热度: the AniEx heat index by day (docs/v3-futures.md §3), the work's (or the works' summed) published values,
// with the episodes under the days they aired, in fixed slots ending on the latest day (a day without a value is an
// empty slot). Days rebuilt from the archives before the live index began are lighter and say 回填. Pointing at (or
// focusing, or tapping) a day shows it in the readout; without a choice the readout shows the latest day and its
// change on the week before. ←/→ move.
import { computed, ref, watch } from 'vue'
import type { StockDetail } from '../../api'
import { addDays, dayLabel, dir, num, pct } from '../../format'

type Day = { day: string; index: number; backfill: boolean }
const props = defineProps<{ s: StockDetail; days: Day[] | null; failed: boolean; today?: string; desk: boolean }>()
const emit = defineEmits<{ retry: [] }>()

const n = computed(() => (props.desk ? 56 : 28))
/** n slots ending on the latest day: the value, or null for a day without one */
const shown = computed<{ day: string; v: Day | null }[]>(() => {
  const all = props.days ?? []
  if (!all.length) return []
  const by = new Map(all.map((d) => [d.day, d]))
  const last = all[all.length - 1].day
  return Array.from({ length: n.value }, (_, i) => {
    const day = addDays(last, i - n.value + 1)
    return { day, v: by.get(day) ?? null }
  })
})
const max = computed(() => Math.max(1, ...shown.value.map((d) => d.v?.index ?? 0)))
const eps = computed(() => new Map(props.s.episode_days.map((e) => [e.trading_day, e.episode])))

const picked = ref<number | null>(null)
const hovering = ref(false)
watch(() => props.days?.length, () => (picked.value = null))
const cur = computed(() => picked.value ?? shown.value.length - 1)
const d = computed(() => shown.value[cur.value]?.v ?? null)
/** the mean of the 7 days to `i` over the 7 before them */
const week = computed(() => {
  const all = props.days ?? []
  const i = all.length - 1
  if (i < 13) return null
  const avg = (a: number, b: number) => all.slice(a, b).reduce((x, y) => x + y.index, 0) / (b - a)
  const before = avg(i - 13, i - 6)
  return before > 0 ? avg(i - 6, i + 1) / before - 1 : null
})

/** the averages beside the chart (desk) and under it (phone) */
const facts = computed(() => {
  const all = props.days ?? []
  const avg = (k: number) => (all.length >= k ? all.slice(-k).reduce((x, y) => x + y.index, 0) / k : null)
  const peak = all.reduce<{ day: string; index: number } | null>((m, x) => (!m || x.index > m.index ? x : m), null)
  return [
    { k: '近 7 天平均', v: avg(7) != null ? num(avg(7)) : '–' },
    { k: '近 28 天平均', v: avg(28) != null ? num(avg(28)) : '–' },
    { k: '最高', v: peak ? `${num(peak.index)}（${dayLabel(peak.day, props.today)}）` : '–' },
  ]
})

const bars = ref<HTMLElement | null>(null)
function onKey(e: KeyboardEvent) {
  const len = shown.value.length
  let j = -1
  if (e.key === 'ArrowRight') j = Math.min(len - 1, cur.value + 1)
  else if (e.key === 'ArrowLeft') j = Math.max(0, cur.value - 1)
  else if (e.key === 'Home') j = 0
  else if (e.key === 'End') j = len - 1
  if (j < 0) return
  e.preventDefault()
  picked.value = j
  bars.value?.querySelectorAll<HTMLElement>('.col')[j]?.focus()
}
function leave() {
  hovering.value = false
  if (!bars.value?.contains(document.activeElement)) picked.value = null
}
</script>

<template>
  <div class="heat">
    <p v-if="failed && !days" class="none">无法加载热度。<button type="button" class="linkbtn" @click="emit('retry')">重试</button></p>
    <div v-else-if="!days" class="skel" style="height: 160px"></div>
    <p v-else-if="!days.length" class="none">{{ s.episodes_aired ? '暂无热度' : '开播后每天公布热度' }}</p>
    <div v-else class="top">
      <div class="chart">
      <div class="read" aria-live="polite">
        <span class="dl">{{ shown[cur] ? dayLabel(shown[cur].day, today) : '' }}<template v-if="d?.backfill"> · 回填</template></span>
        <span class="num v">{{ d ? num(d.index) : '–' }}</span>
        <span v-if="picked == null && week != null" class="num wk" :class="dir(week)">比上周 {{ pct(week, 1) }}</span>
      </div>
      <div ref="bars" class="bars" :class="{ dim: picked != null || hovering }" role="group" aria-label="每日热度"
           @pointerleave="leave" @keydown="onKey">
        <button v-for="(x, i) in shown" :key="x.day" type="button" class="col" :class="{ on: i === cur }"
                :tabindex="i === cur ? 0 : -1"
                :aria-label="`${dayLabel(x.day, today)}，${x.v ? `热度 ${num(x.v.index)}${x.v.backfill ? '（回填）' : ''}` : '没有数据'}${eps.has(x.day) ? `，第 ${eps.get(x.day)} 集播出` : ''}`"
                :aria-pressed="i === cur" @pointerenter="hovering = true; picked = i" @focus="picked = i" @click="picked = i">
          <span v-if="x.v" class="bar" :class="{ bf: x.v.backfill }" :style="{ height: `calc((100% - 18px) * ${x.v.index / max})` }"></span>
          <span class="ep num">{{ eps.get(x.day) ?? '' }}</span>
        </button>
      </div>
      </div>
      <dl class="facts kpis">
        <div v-for="f in facts" :key="f.k"><dt>{{ f.k }}</dt><dd class="num">{{ f.v }}</dd></div>
      </dl>
    </div>
  </div>
</template>

<style scoped>
.heat, .top, .chart { display: flex; flex-direction: column; gap: 10px; }
.top { gap: 14px; }
.kpis { border-top: 1px solid var(--hair); }
.none { font-size: 13px; color: var(--muted); padding: 8px 0; }
.read { display: flex; align-items: baseline; gap: 10px; min-height: 30px; }
.dl { font-size: 12px; color: var(--muted); }
.v { font-size: 22px; font-weight: 600; color: var(--ink); }
.wk { font-size: 13px; }
.bars { position: relative; display: flex; align-items: stretch; gap: 3px; height: 150px; touch-action: pan-y; }
.bars::after { content: ''; position: absolute; left: 0; right: 0; bottom: 17px; border-top: 1px solid var(--rule); pointer-events: none; }
.col {
  flex: 1; min-width: 0; display: flex; flex-direction: column; justify-content: flex-end; gap: 4px; height: 100%;
  border: 0; padding: 0; background: transparent; cursor: pointer; transition: opacity var(--dur-1) var(--ease-out);
}
.col:focus-visible { outline: 2px solid var(--ink); outline-offset: 2px; }
.dim .col:not(.on) { opacity: 0.4; }
.bar { display: block; background: var(--ink); min-height: 1px; transition: height var(--dur-2) var(--ease-out); }
.bar.bf { background: var(--muted); opacity: 0.55; }
.ep { height: 12px; font-size: 9px; line-height: 12px; color: var(--muted); text-align: center; padding-top: 2px; box-sizing: content-box; }
@media (min-width: 1024px) {
  .top { display: grid; grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr); gap: 32px; align-items: start; }
  .kpis { border-top: 0; }
  .kpis > div { border-bottom: 1px solid var(--hair-2); }
  .bars { height: 180px; }
}
@media (prefers-reduced-motion: reduce) { .col, .bar { transition: none; } }
</style>
