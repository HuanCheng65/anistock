<script setup lang="ts">
import { computed, ref } from 'vue'
import { api, call, errorText, type MacroRow } from '../api'
import { usePoll } from '../composables/usePoll'
import BackButton from '../components/BackButton.vue'
import EmptyState from '../components/EmptyState.vue'
import { bigMoney, pct } from '../format'
import { useSession } from '../stores/session'

const session = useSession()
/** the closed days (rows with the central bank's figures) */
const days = ref<(MacroRow & { policy_rate: number; m2: number })[] | null>(null)
const error = ref('')
usePoll(async () => {
  try {
    const rows = (await call(api.GET('/api/v1/market/macro', { params: { query: { from: '2000-01-01' } } }))).items
    days.value = rows.filter((r): r is MacroRow & { policy_rate: number; m2: number } => r.policy_rate != null && r.m2 != null)
    error.value = ''
  } catch (e) {
    error.value = errorText(e, '无法加载')
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
const rate = computed(() => line((days.value ?? []).map((d) => d.policy_rate * 100)))
const m2 = computed(() => line((days.value ?? []).map((d) => d.m2)))
const last = computed(() => days.value?.[days.value.length - 1])
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton fallback="/" />
      <h1>宏观</h1>
    </header>
    <dl class="facts g4 gutter">
      <div><dt>政策利率</dt><dd>{{ session.status ? (session.status.policy_rate * 100).toFixed(2) + '%' : '–' }}</dd></div>
      <div><dt>存款利率</dt><dd>{{ session.status ? (session.status.deposit_rate * 100).toFixed(2) + '%' : '–' }}</dd></div>
      <div><dt>M2 年化增速</dt><dd>{{ last?.m2_growth != null ? pct(last.m2_growth, 1) : '–' }}</dd></div>
      <div><dt>货币供应量</dt><dd>{{ last ? bigMoney(last.m2) : '–' }}</dd></div>
    </dl>
    <p v-if="error" class="msg error gutter">{{ error }}</p>
    <EmptyState v-if="days && days.length < 2" text="每次日结记录一次宏观数据，数日后显示走势" />
    <template v-else-if="days">
      <section class="sec">
        <h2>政策利率</h2>
        <svg v-if="rate" :viewBox="`0 0 ${W} ${H}`" class="chart" role="img" aria-label="政策利率走势">
          <text :x="L - 6" :y="rate.y(rate.hi) + 4" class="axis" text-anchor="end">{{ rate.hi.toFixed(2) }}%</text>
          <text :x="L - 6" :y="rate.y(rate.lo) + 4" class="axis" text-anchor="end">{{ rate.lo.toFixed(2) }}%</text>
          <line :x1="L" :x2="W" :y1="rate.y(rate.lo)" :y2="rate.y(rate.lo)" class="baseline" />
          <path :d="rate.d" fill="none" class="l1" stroke-width="2" />
          <text :x="L" :y="H - 4" class="axis">{{ days[0].trading_day.slice(5) }}</text>
          <text :x="W - 4" :y="H - 4" class="axis" text-anchor="end">{{ days[days.length - 1].trading_day.slice(5) }}</text>
        </svg>
      </section>
      <section class="sec">
        <h2>货币供应量</h2>
        <svg v-if="m2" :viewBox="`0 0 ${W} ${H}`" class="chart" role="img" aria-label="货币供应量走势">
          <text :x="L - 6" :y="m2.y(m2.hi) + 4" class="axis" text-anchor="end">{{ bigMoney(m2.hi) }}</text>
          <text :x="L - 6" :y="m2.y(m2.lo) + 4" class="axis" text-anchor="end">{{ bigMoney(m2.lo) }}</text>
          <line :x1="L" :x2="W" :y1="m2.y(m2.lo)" :y2="m2.y(m2.lo)" class="baseline" />
          <path :d="m2.d" fill="none" class="l2" stroke-width="2" />
          <text :x="L" :y="H - 4" class="axis">{{ days[0].trading_day.slice(5) }}</text>
          <text :x="W - 4" :y="H - 4" class="axis" text-anchor="end">{{ days[days.length - 1].trading_day.slice(5) }}</text>
        </svg>
      </section>
    </template>
  </main>
</template>

<style scoped>
.g4 { grid-template-columns: repeat(2, minmax(0, 1fr)); border-bottom: 1px solid var(--hair); }
@media (min-width: 720px) { .g4 { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
.chart { width: 100%; height: auto; display: block; }
.axis { font-size: 11px; fill: var(--muted); font-family: var(--font-num); }
.baseline { stroke: var(--hair); }
.l1 { stroke: var(--ink); }
.l2 { stroke: var(--mk-cn); }
</style>
