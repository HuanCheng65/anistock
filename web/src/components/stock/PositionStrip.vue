<script setup lang="ts">
// 持仓 on the stock page: the player's shares here and, with 融资融券, the financing owed and the shares owed.
// Shown only when there is something.
import { computed } from 'vue'
import type { CreditAccount, Position } from '../../api'
import { dir, money, pct, qty, signed } from '../../format'

const props = defineProps<{ pos: Position | null; credit: CreditAccount | null; stockId: number }>()

const debt = computed(() => props.credit?.debts.find((d) => d.stock.id === props.stockId) ?? null)
const short = computed(() => props.credit?.shorts.find((x) => x.stock.id === props.stockId) ?? null)
const has = computed(() => (props.pos?.quantity ?? 0) > 0 || !!debt.value || !!short.value)
</script>

<template>
  <section v-if="has" class="pos" aria-labelledby="pos-h">
    <h2 id="pos-h" class="h2">持仓</h2>
    <dl class="facts pf">
      <template v-if="pos && pos.quantity > 0">
        <div><dt>数量</dt><dd v-flash.neutral="pos.quantity">{{ qty(pos.quantity) }}</dd></div>
        <div><dt>成本</dt><dd>{{ money(pos.average_cost) }}</dd></div>
        <div><dt>市值</dt><dd v-flash.neutral="pos.value">{{ money(pos.value) }}</dd></div>
        <div class="pl">
          <dt>盈亏</dt>
          <dd :class="dir(pos.pnl)">{{ signed(pos.pnl) }}<span class="r">{{ pct(pos.pnl_ratio) }}</span></dd>
        </div>
        <div><dt>累计分红</dt><dd>{{ money(pos.dividends) }}</dd></div>
      </template>
      <div v-if="debt"><dt>融资负债</dt><dd>{{ money(debt.principal + debt.interest) }}</dd></div>
      <div v-if="short"><dt>欠券</dt><dd>{{ qty(short.quantity) }}</dd></div>
    </dl>
  </section>
</template>

<style scoped>
.pos { padding: 14px var(--gutter) 4px; border-top: 1px solid var(--rule); display: flex; flex-direction: column; gap: 4px; }
.pl dd { display: flex; flex-wrap: wrap; column-gap: 8px; align-items: baseline; }
.pl .r { font-size: 12px; }
@media (min-width: 1024px) {
  .pos { flex-direction: row; align-items: center; gap: 28px; padding: 10px var(--gutter); }
  .pos .h2 { flex: none; }
  .pf { flex: 1; grid-template-columns: repeat(auto-fit, minmax(96px, 1fr)); }
}
</style>
