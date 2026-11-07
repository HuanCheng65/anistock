<script setup lang="ts">
// 持仓 on the stock page: the player's shares here and, with 融资融券, the financing owed and the shares owed.
// Shown only when there is something. 盈亏 counts the dividends received: a season pays its value out week by
// week, so the price alone reads as a loss when it is not; the line under it splits the two.
import { computed } from 'vue'
import type { CreditAccount, Position } from '../../api'
import { dir, lowestTaxIn, money, pct, qty, signed } from '../../format'
import { useSession } from '../../stores/session'

const props = defineProps<{ pos: Position | null; credit: CreditAccount | null; stockId: number }>()

const debt = computed(() => props.credit?.debts.find((d) => d.stock.id === props.stockId) ?? null)
const short = computed(() => props.credit?.shorts.find((x) => x.stock.id === props.stockId) ?? null)
const session = useSession()
const total = computed(() => (props.pos ? props.pos.pnl + props.pos.dividends : 0))
const taxIn = computed(() => lowestTaxIn(props.pos?.newest_lot_on, session.status?.trading_day, session.rules?.dividend_tax))
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
          <dd :class="dir(total)">{{ signed(total) }}<span class="r">{{ pct(pos.pnl_ratio == null ? null : total / (pos.value - pos.pnl)) }}</span></dd>
        </div>
        <div class="split">
          <dt>股价 / 分红</dt>
          <dd><span :class="dir(pos.pnl)">{{ signed(pos.pnl) }}</span> / <span :class="dir(pos.dividends)">{{ signed(pos.dividends) }}</span></dd>
        </div>
      </template>
      <div v-if="debt"><dt>融资负债</dt><dd>{{ money(debt.principal + debt.interest) }}</dd></div>
      <div v-if="short"><dt>欠券</dt><dd>{{ qty(short.quantity) }}</dd></div>
    </dl>
    <p v-if="pos && pos.quantity > 0 && taxIn > 0" class="tax">再持有 {{ taxIn }} 天，分红免税</p>
  </section>
</template>

<style scoped>
.pos { padding: 14px var(--gutter) 4px; border-top: 1px solid var(--rule); display: flex; flex-direction: column; gap: 4px; }
.pl dd { display: flex; flex-wrap: wrap; column-gap: 8px; align-items: baseline; }
.pl .r { font-size: 12px; }
.tax { font-size: 12px; color: var(--muted); padding-bottom: 6px; }
@media (min-width: 1024px) {
  .pos { flex-direction: row; align-items: center; gap: 28px; padding: 10px var(--gutter); }
  .pos .h2 { flex: none; }
  .pf { flex: 1; grid-template-columns: repeat(auto-fit, minmax(96px, 1fr)); }
}
</style>
