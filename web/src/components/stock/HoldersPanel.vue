<script setup lang="ts">
// 股东: who holds the shares (制作委员会, 玩家, NPC, 做市商; NPCs as one group), and the stock's 融资融券: the
// balance financed, the shares lent out, the shares still lendable, or why it is not a 融资融券 stock.
import { computed } from 'vue'
import type { StockDetail, StockMargin } from '../../api'
import { bigMoney, qty, share } from '../../format'

const props = defineProps<{ s: StockDetail; margin: StockMargin | null }>()

const holders = computed(() => {
  const h = props.s.holders
  const total = props.s.shares || 1
  return [
    { k: '制作委员会', n: h.committee, c: 'var(--hold-committee)' },
    { k: '玩家', n: h.player, c: 'var(--hold-player)' },
    { k: 'NPC', n: h.npc.total, c: 'var(--hold-npc)' },
    { k: '做市商', n: h.market_maker, c: 'var(--hold-mm)' },
    // the company's own account: shares not yet sold at the IPO, later the ones it bought back
    { k: props.s.status === 'subscribing' ? '待发行' : '公司持有', n: h.unissued, c: 'var(--hold-unissued)' },
    { k: '其他', n: h.other, c: 'var(--hold-unissued)' },
  ]
    .map((x) => ({ ...x, v: x.n / total }))
    .filter((x) => x.n > 0 || x.k === '玩家')
})

const REASON: Record<string, string> = {
  new_listing: '新上市的股票暂不能融资融券',
  penny: '股价过低，不能融资融券',
  delisting: '处于退市倒计时，不能融资融券',
  not_listed: '上市后才能融资融券',
}
const marginFacts = computed(() => {
  const m = props.margin
  if (!m) return []
  return [
    { k: '融资余额', v: bigMoney(m.financed) },
    { k: '融券余量', v: qty(m.shorted) },
    { k: '可融券数量', v: qty(m.lendable) },
    { k: '融资利率', v: share(m.financing_rate, 2) },
    { k: '融券费率', v: share(m.lending_rate, 2) },
    { k: '折算率', v: share(m.haircut) },
  ]
})
</script>

<template>
  <div class="holders">
    <section class="blk" aria-label="持股">
      <div class="hbar" role="img" :aria-label="holders.map((h) => `${h.k} ${share(h.v, 1)}`).join('，')">
        <span v-for="h in holders" :key="h.k" :style="{ flexGrow: Math.max(h.v, 0.0001), background: h.c }"></span>
      </div>
      <ul class="hleg">
        <li v-for="h in holders" :key="h.k">
          <i :style="{ background: h.c }"></i>
          <span class="k">{{ h.k }}</span>
          <span class="num q">{{ qty(h.n) }}</span>
          <span class="num v">{{ share(h.v, 1) }}</span>
        </li>
      </ul>
    </section>

    <section class="blk" aria-labelledby="h-margin">
      <h3 id="h-margin" class="h3">融资融券</h3>
      <template v-if="margin">
        <p v-if="!margin.eligible" class="none">{{ REASON[margin.reason ?? ''] ?? '不是融资融券标的' }}</p>
        <dl class="facts mf">
          <div v-for="f in marginFacts" :key="f.k"><dt>{{ f.k }}</dt><dd>{{ f.v }}</dd></div>
        </dl>
      </template>
      <div v-else class="skel" style="height: 56px"></div>
    </section>
  </div>
</template>

<style scoped>
.holders { display: flex; flex-direction: column; gap: 22px; }
.blk { display: flex; flex-direction: column; gap: 10px; }
.h3 { font-family: var(--font-serif); font-size: 15px; font-weight: 900; }
.hbar { display: flex; height: 12px; gap: 2px; }
.hbar span { flex-basis: 0; transition: flex-grow var(--dur-3) var(--ease-out); }
.hleg li { display: grid; grid-template-columns: 10px minmax(0, 1fr) auto 64px; gap: 10px; align-items: baseline; padding: 8px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
.hleg i { width: 10px; height: 10px; align-self: center; }
.hleg .k { color: var(--ink-2); }
.hleg .q { color: var(--muted); font-size: 13px; }
.hleg .v { text-align: right; }
.none { font-size: 13px; color: var(--ink-2); }
.mf { border-top: 1px solid var(--hair); }
@media (min-width: 1024px) {
  .holders { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 32px; align-items: start; }
}
@media (prefers-reduced-motion: reduce) { .hbar span { transition: none; } }
</style>
