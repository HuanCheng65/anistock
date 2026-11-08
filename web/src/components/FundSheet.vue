<script setup lang="ts">
// 申购 / 赎回 one fund (docs/design/funds.md §5): an amount to subscribe or units to redeem, when it executes and is
// confirmed, an estimate at the last net value, and the redemption fee on units held under 7 days (the oldest
// units go first). A request can be cancelled from 我的 until it starts executing.
import { computed, ref, watch } from 'vue'
import { errorText, type FundView } from '../api'
import { addDays, money, num, toCents } from '../format'
import { useFunds } from '../stores/funds'
import { useSession } from '../stores/session'
import { useToast } from '../stores/toast'
import Modal from './Modal.vue'

const props = defineProps<{ open: boolean; fund: FundView; side: 'buy' | 'sell'; minSubscribe: number; redeemFee: number; feeDays: number }>()
const emit = defineEmits<{ close: [] }>()
const funds = useFunds()
const session = useSession()
const toast = useToast()

const side = ref<'buy' | 'sell'>(props.side)
const text = ref('')
const busy = ref(false)
const error = ref('')
watch(() => [props.open, props.side] as const, ([o, s]) => {
  if (o) {
    side.value = s
    text.value = ''
    error.value = ''
  }
})
watch(side, () => {
  text.value = ''
  error.value = ''
})

const buying = computed(() => side.value === 'buy')
const cash = computed(() => session.me?.available_cash ?? 0)
const hold = computed(() => funds.mine?.holdings.find((h) => h.fund.key === props.fund.key) ?? null)
const held = computed(() => hold.value?.units ?? 0)
/** cents to subscribe, or hundredths of a unit to redeem (both two decimals) */
const value = computed(() => toCents(text.value) ?? 0)
const quick = computed(() =>
  buying.value
    ? [[100_000, '1,000'], [500_000, '5,000'], [1_000_000, '1万'], [cash.value, '全部']]
    : [[Math.floor(held.value / 4), '1/4'], [Math.floor(held.value / 2), '一半'], [Math.floor((held.value * 3) / 4), '3/4'], [held.value, '全部']],
)
function pick(v: number) {
  text.value = (v / 100).toFixed(2).replace(/\.00$/, '')
}

// the session it executes in: before 08:50 the day's own, after it the next (the trading day changes at 04:00)
const execDay = computed(() => {
  const st = session.status
  if (!st) return null
  // the time in UTC+8, the game's
  const t = new Date(Date.now() + 8 * 3600_000).toISOString().slice(11, 16)
  return t >= '04:00' && t < '08:50' ? st.trading_day : addDays(st.trading_day, 1)
})
const md = (d: string | null) => (d ? `${d.slice(5, 7)}-${d.slice(8)}` : '')
const today = computed(() => md(session.status?.trading_day ?? null))

const youngOut = computed(() => Math.max(0, value.value - (held.value - (hold.value?.young_units ?? 0))))
const fee = computed(() => Math.round(youngOut.value * props.fund.nav * props.redeemFee))
const estimate = computed(() =>
  buying.value ? Math.round(value.value / props.fund.nav) : Math.round(value.value * props.fund.nav),
)
const problem = computed(() => {
  if (!value.value) return ''
  if (buying.value) {
    if (value.value < props.minSubscribe) return `申购金额不能低于 ${money(props.minSubscribe, 0)} 星辉`
    if (value.value > cash.value) return '可用资金不足'
  } else if (value.value > held.value) return '可赎回的份额不足'
  return ''
})

async function submit() {
  if (!value.value || problem.value) return
  busy.value = true
  error.value = ''
  try {
    if (buying.value) await funds.subscribe(props.fund.key, value.value)
    else await funds.redeem(props.fund.key, value.value)
    session.loadMe().catch(() => {})
    toast.show({ key: 'fund', text: buying.value ? '已提交申购' : '已提交赎回', sub: `${md(execDay.value)} 成交，收盘后确认`, tone: 'plain' })
    emit('close')
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <Modal :open="open" :title="fund.name" edit-key="fund" :busy="busy" @close="emit('close')">
    <div class="sides" role="group" aria-label="方向">
      <button type="button" :aria-pressed="buying" @click="side = 'buy'">申购</button>
      <button type="button" :aria-pressed="!buying" :disabled="!held" @click="side = 'sell'">赎回</button>
    </div>
    <label class="field">
      <span class="lbl"><span>{{ buying ? '金额' : '份额' }}</span><span class="num">{{ buying ? `可用资金 ${money(cash)}` : `持有 ${money(held)} 份` }}</span></span>
      <span class="box">
        <input v-model="text" class="num" inputmode="decimal" autocomplete="off" autofocus :aria-invalid="!!problem" />
        <span class="unit">{{ buying ? '星辉' : '份' }}</span>
      </span>
    </label>
    <div class="quick">
      <button v-for="[v, l] in quick" :key="l" type="button" @click="pick(v as number)">{{ l }}</button>
    </div>
    <ol class="steps">
      <li><span>提交</span>{{ today }}</li>
      <li><span>成交</span>{{ md(execDay) }} 盘中分批</li>
      <li><span>确认</span>{{ md(execDay) }} 收盘后按当天的净值确认{{ buying ? '份额' : '金额' }}</li>
    </ol>
    <p class="explain">
      <template v-if="buying">份额在确认时才能算出。<template v-if="value">按最新净值 <span class="num">{{ num(fund.nav, 4) }}</span> 估算约 <span class="num">{{ money(estimate) }}</span> 份。</template>基金当天的买入成本从申购金额中扣除。</template>
      <template v-else>金额在确认时才能算出。<template v-if="value">按最新净值 <span class="num">{{ num(fund.nav, 4) }}</span> 估算约 <span class="num">{{ money(estimate) }}</span> 星辉。</template>基金当天的卖出成本从赎回金额中扣除。</template>
    </p>
    <p v-if="!buying && youngOut > 0" class="fee">其中 <span class="num">{{ money(youngOut) }}</span> 份持有不满 {{ feeDays }} 天，赎回费约 <span class="num">{{ money(fee) }}</span>。<template v-if="hold?.free_from">{{ md(hold.free_from) }} 起赎回不收。</template></p>
    <p v-if="problem || error" class="msg error" role="alert">{{ problem || error }}</p>
    <template #actions>
      <button type="button" class="btn" :disabled="busy || !value || !!problem" @click="submit">
        {{ busy ? '提交中' : buying ? `申购 ${value ? money(value, 0) : ''}`.trim() : `赎回 ${value ? money(value) + ' 份' : ''}`.trim() }}
      </button>
      <button type="button" class="btn ghost" :disabled="busy" @click="emit('close')">取消</button>
    </template>
  </Modal>
</template>

<style scoped>
.sides { display: grid; grid-template-columns: 1fr 1fr; border: 1px solid var(--ink); margin-bottom: 14px; }
.sides button { height: 40px; border: 0; background: none; font-size: 15px; color: var(--ink); }
.sides button[aria-pressed="true"] { background: var(--ink); color: var(--ground); font-weight: 700; }
.field { display: flex; flex-direction: column; gap: 6px; }
.lbl { display: flex; justify-content: space-between; font-size: 13px; color: var(--muted); }
.box { display: flex; align-items: center; border: 1px solid var(--ink); height: 48px; padding: 0 12px; background: var(--ground); }
.box input { flex: 1; min-width: 0; border: 0; background: none; font-size: 22px; font-weight: 600; color: var(--ink); outline: none; }
.unit { font-size: 14px; color: var(--muted); }
.quick { display: flex; gap: 8px; margin: 10px 0 14px; }
.quick button { flex: 1; height: 32px; border: 1px solid var(--base-line); background: none; font-size: 13px; color: var(--ink); }
.steps { margin: 0; padding: 12px; list-style: none; background: var(--ground-2); display: flex; flex-direction: column; gap: 6px; font-size: 13px; line-height: 1.6; }
.steps span { display: inline-block; width: 56px; color: var(--muted); }
.explain { margin: 12px 0 0; font-size: 13px; line-height: 1.6; }
.fee { margin: 8px 0 0; font-size: 13px; line-height: 1.6; color: var(--danger); }
.msg { margin-top: 8px; }
</style>
