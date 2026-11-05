<script setup lang="ts">
// Weekly revenue, stacked by market (us bottom, cn, jp top), with the market split of the period.
import { computed } from 'vue'
import type { WeekRow } from '../api'
import { big, share } from '../format'
const props = withDefaults(defineProps<{ weeks: WeekRow[]; n?: number; h?: number }>(), { n: 8, h: 96 })
const shown = computed(() => props.weeks.slice(-props.n))
const max = computed(() => Math.max(1, ...shown.value.map((w) => w.us + w.cn + w.jp)))
const split = computed(() => {
  const t = shown.value.reduce((a, w) => ({ us: a.us + w.us, cn: a.cn + w.cn, jp: a.jp + w.jp }), { us: 0, cn: 0, jp: 0 })
  const s = t.us + t.cn + t.jp || 1
  return { us: t.us / s, cn: t.cn / s, jp: t.jp / s }
})
const px = (v: number) => Math.max(0, (v / max.value) * (props.h - 6)) + 'px'
</script>

<template>
  <div class="rev">
    <div class="bars" :style="{ height: h + 'px' }" role="img" :aria-label="`近 ${shown.length} 周营收，欧美 ${share(split.us)}，中国 ${share(split.cn)}，日本 ${share(split.jp)}`">
      <div v-for="w in shown" :key="w.week" class="col" :title="`${w.week} 起一周 ${big(w.revenue)}`">
        <span class="jp" :style="{ height: px(w.jp) }"></span>
        <span class="cn" :style="{ height: px(w.cn) }"></span>
        <span class="us" :style="{ height: px(w.us) }"></span>
      </div>
    </div>
    <div class="legend">
      <span><i class="us"></i>欧美 <b class="num">{{ share(split.us) }}</b></span>
      <span><i class="cn"></i>中国 <b class="num">{{ share(split.cn) }}</b></span>
      <span><i class="jp"></i>日本 <b class="num">{{ share(split.jp) }}</b></span>
    </div>
  </div>
</template>

<style scoped>
.rev { display: flex; flex-direction: column; gap: 12px; }
.bars { display: flex; align-items: flex-end; gap: 6px; }
.col { flex: 1; display: flex; flex-direction: column; gap: 2px; justify-content: flex-end; height: 100%; }
.us { background: var(--mk-us); }
.cn { background: var(--mk-cn); }
.jp { background: var(--mk-jp); }
.legend { display: flex; gap: 16px; font-size: 12px; color: var(--ink-2); flex-wrap: wrap; }
.legend span { display: flex; align-items: center; gap: 6px; }
.legend i { width: 10px; height: 10px; display: inline-block; }
.legend b { font-weight: 400; color: var(--ink); }
</style>
