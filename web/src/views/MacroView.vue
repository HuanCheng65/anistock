<script setup lang="ts">
import { computed, ref } from 'vue'
import { api, ApiError, type MacroDay } from '../api'
import { usePoll } from '../composables/usePoll'
import { big, pct, shortDate } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
const days = ref<MacroDay[] | null>(null)
const error = ref('')
usePoll(async () => {
  try {
    days.value = (await api.get<{ days: MacroDay[] }>('/api/macro')).days
    error.value = ''
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '加载失败'
  }
}, 60000)

const W = 600, H = 150, L = 46, B = 20, T = 8
function line(vals: number[]) {
  if (vals.length < 2) return null
  const lo = Math.min(...vals), hi = Math.max(...vals)
  const span = hi - lo || Math.abs(hi) * 0.1 || 1
  const x = (i: number) => L + (i / (vals.length - 1)) * (W - L - 6)
  const y = (v: number) => T + (H - T - B) * (1 - (v - lo) / span)
  return { d: vals.map((v, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(''), lo, hi, y }
}
const rate = computed(() => line((days.value ?? []).map((d) => d.rate * 100)))
const m2 = computed(() => line((days.value ?? []).map((d) => d.m2)))
const last = computed(() => days.value?.[days.value.length - 1])
</script>

<template>
  <main class="page">
    <section class="panel stats">
      <div><span class="label">政策利率</span><span class="num big">{{ session.status ? (session.status.rate * 100).toFixed(2) + '%' : '–' }}</span></div>
      <div><span class="label">存款利率</span><span class="num big">{{ session.status ? (session.status.deposit_rate * 100).toFixed(2) + '%' : '–' }}</span></div>
      <div><span class="label">M2 年化增速</span><span class="num big">{{ last?.m2_growth != null ? pct(last.m2_growth, 1) : '–' }}</span></div>
      <div><span class="label">货币供应量</span><span class="num big">{{ last ? big(last.m2) : '–' }}</span></div>
    </section>
    <p v-if="error" class="notice error">{{ error }}</p>
    <p v-if="days && days.length < 2" class="empty panel">开市后每日结算都会记录一次宏观数据，过几天这里就会有曲线。</p>
    <template v-else-if="days">
      <section class="panel">
        <div class="panel-head"><h2>政策利率</h2><span class="small muted">每周日决议</span></div>
        <svg v-if="rate" :viewBox="`0 0 ${W} ${H}`" class="chart" role="img" aria-label="政策利率走势">
          <text :x="L - 6" :y="rate.y(rate.hi) + 4" class="axis" text-anchor="end">{{ rate.hi.toFixed(2) }}%</text>
          <text :x="L - 6" :y="rate.y(rate.lo) + 4" class="axis" text-anchor="end">{{ rate.lo.toFixed(2) }}%</text>
          <line :x1="L" :x2="W" :y1="rate.y(rate.lo)" :y2="rate.y(rate.lo)" stroke="var(--rule)" />
          <path :d="rate.d" fill="none" stroke="var(--accent)" stroke-width="2" />
          <text :x="L" :y="H - 4" class="axis">{{ shortDate(days[0].day) }}</text>
          <text :x="W - 4" :y="H - 4" class="axis" text-anchor="end">{{ shortDate(days[days.length - 1].day) }}</text>
        </svg>
      </section>
      <section class="panel">
        <div class="panel-head"><h2>货币供应量 M2</h2><span class="small muted">玩家、NPC 与公司账上的现金</span></div>
        <svg v-if="m2" :viewBox="`0 0 ${W} ${H}`" class="chart" role="img" aria-label="货币供应量走势">
          <text :x="L - 6" :y="m2.y(m2.hi) + 4" class="axis" text-anchor="end">{{ big(m2.hi) }}</text>
          <text :x="L - 6" :y="m2.y(m2.lo) + 4" class="axis" text-anchor="end">{{ big(m2.lo) }}</text>
          <line :x1="L" :x2="W" :y1="m2.y(m2.lo)" :y2="m2.y(m2.lo)" stroke="var(--rule)" />
          <path :d="m2.d" fill="none" stroke="var(--s-cn)" stroke-width="2" />
          <text :x="L" :y="H - 4" class="axis">{{ shortDate(days[0].day) }}</text>
          <text :x="W - 4" :y="H - 4" class="axis" text-anchor="end">{{ shortDate(days[days.length - 1].day) }}</text>
        </svg>
      </section>
    </template>
  </main>
</template>

<style scoped>
.stats { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; padding: 12px; }
@media (min-width: 720px) { .stats { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
.stats div { display: flex; flex-direction: column; }
.big { font-size: 20px; font-weight: 700; }
.chart { width: 100%; height: auto; display: block; padding: 8px 4px; }
.axis { font-size: 11px; fill: var(--muted); font-family: var(--font-num); }
</style>
