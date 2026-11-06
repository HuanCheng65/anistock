<script setup lang="ts">
// 经营: weekly revenue by market (per-share profit and dividend per week in the readout), the last four weeks'
// actual per-share profit and dividends, the trailing ratios, and every dividend paid. Only what happened:
// no annualised figures.
import { computed, ref } from 'vue'
import type { StockDetail } from '../../api'
import { money, share, shortDay } from '../../format'
import RevenueBars from '../RevenueBars.vue'

const props = defineProps<{ s: StockDetail; today?: string; desk: boolean }>()

const r = computed(() => props.s.ratios)
const facts = computed(() => [
  { k: '近 4 周每股收益', v: money(props.s.trailing_4w.earnings_per_share) },
  { k: '近 4 周每股分红', v: money(props.s.trailing_4w.dividends_per_share) },
  { k: '市盈率(TTM)', v: r.value.pe_ttm != null ? r.value.pe_ttm.toFixed(1) : '–' },
  { k: '股息率(TTM)', v: r.value.dividend_yield_ttm != null ? share(r.value.dividend_yield_ttm, 1) : '–' },
  { k: '市净率', v: r.value.pb != null ? r.value.pb.toFixed(2) : '–' },
  { k: '每股净资产', v: money(r.value.book_value_per_share) },
])

const SHOW = 8
const all = ref(false)
const divs = computed(() => [...props.s.dividends].reverse())
const shownDivs = computed(() => (all.value ? divs.value : divs.value.slice(0, SHOW)))
</script>

<template>
  <div class="biz">
    <div class="top">
      <div class="bars">
        <RevenueBars v-if="s.revenue_weeks.length" :weeks="s.revenue_weeks" :n="desk ? 12 : 8" :h="desk ? 132 : 104" :today="today" />
        <p v-else class="none">{{ s.status === 'subscribing' ? '上市后每天公布营收' : '暂无营收' }}</p>
      </div>
      <dl class="facts kpis">
        <div v-for="f in facts" :key="f.k"><dt>{{ f.k }}</dt><dd>{{ f.v }}</dd></div>
      </dl>
    </div>

    <section class="divs" aria-labelledby="biz-divs">
      <h3 id="biz-divs" class="h3">分红</h3>
      <ol v-if="divs.length" class="dlist">
        <li v-for="d in shownDivs" :key="d.trading_day">
          <span class="num d">{{ shortDay(d.trading_day, today) }}</span>
          <span class="num v">{{ money(d.per_share) }}</span>
        </li>
      </ol>
      <p v-else class="none">暂无分红</p>
      <button v-if="divs.length > SHOW" type="button" class="linkbtn more" :aria-expanded="all" @click="all = !all">
        {{ all ? '收起' : `展开全部 ${divs.length} 次` }}
      </button>
    </section>
  </div>
</template>

<style scoped>
.biz { display: flex; flex-direction: column; gap: 22px; }
.top { display: flex; flex-direction: column; gap: 14px; }
.kpis { border-top: 1px solid var(--hair); }
.h3 { font-family: var(--font-serif); font-size: 15px; font-weight: 900; }
.none { font-size: 13px; color: var(--muted); padding: 8px 0; }
.divs { display: flex; flex-direction: column; gap: 6px; }
.dlist { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); column-gap: 24px; }
.dlist li { display: flex; justify-content: space-between; align-items: baseline; padding: 8px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
.dlist .d { font-size: 12px; color: var(--muted); }
.more { align-self: flex-start; padding: 6px 0; }
@media (min-width: 1024px) {
  .top { display: grid; grid-template-columns: minmax(0, 1.5fr) minmax(0, 1fr); gap: 32px; align-items: start; }
  .kpis { grid-template-columns: repeat(2, minmax(0, 1fr)); border-top: 0; column-gap: 16px; }
  .kpis > div { border-bottom: 1px solid var(--hair-2); }
  .dlist { grid-template-columns: repeat(4, minmax(0, 1fr)); }
}
</style>
