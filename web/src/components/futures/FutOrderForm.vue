<script setup lang="ts">
// Order entry for a heat futures contract, on the terminal surface like the stock order form: 押更热 (buy) or
// 押更冷 (sell), lots, a price that starts at 对手价, and one line that turns the order into money: "平均到 100
// 赚 230 星辉", the 100 draggable. Margin and commission under it. Futures trade on the credit account's
// margin (docs/v3-futures.md §4.2). Until the futures steps are done (docs/v3-futures-ui.md §4: credit account,
// 风险测评, the knowledge test, the disclosure) the form lists them with links in place of the button; an
// order that only closes a position needs none of them.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { errorText, type FuturesContract, type FuturesDetail, type FuturesPosition } from '../../api'
import { centsText, money, num, qty as fmtQty, toCents } from '../../format'
import { useFutures } from '../../stores/futures'
import { usePrefs } from '../../stores/prefs'
import { useSession } from '../../stores/session'
import { markEditing } from '../../version'
import Modal from '../Modal.vue'

const props = defineProps<{ c: FuturesContract; d: FuturesDetail | null; pos: FuturesPosition | null; initialSide?: 'buy' | 'sell' }>()
const emit = defineEmits<{ placed: [] }>()
const fut = useFutures()
const session = useSession()
const prefs = usePrefs()
const route = useRoute()

const side = ref<'buy' | 'sell'>(props.initialSide ?? 'buy')
watch(() => props.initialSide, (s) => s && (side.value = s))
const lots = ref(1)
const price = ref('')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
const editKey = `fut-order-${Math.random()}`
onBeforeUnmount(() => markEditing(editKey, false))

const p = computed(() => props.c.last_price / 100)
const bestAsk = computed(() => props.d?.asks[0]?.price ?? null)
const bestBid = computed(() => props.d?.bids[0]?.price ?? null)
const counter = computed(() => (side.value === 'buy' ? bestAsk.value : bestBid.value))
function useCounter() {
  price.value = centsText(counter.value ?? props.c.last_price)
}
watch([() => props.c.id, side], useCounter, { immediate: true })
/** the order's price in hundredths of a point */
const px = computed(() => toCents(price.value) ?? NaN)
const crosses = computed(() => counter.value != null && px.value > 0 && (side.value === 'buy' ? px.value >= counter.value : px.value <= counter.value))

/** what one point is worth to one lot, in cents */
const perPoint = computed(() => props.c.tick_value * 100)
const held = computed(() => props.pos?.quantity ?? 0)
/** lots this order closes (the rest opens) */
const closing = computed(() => (held.value * (side.value === 'buy' ? 1 : -1) < 0 ? Math.min(lots.value, Math.abs(held.value)) : 0))
const margin = computed(() => (lots.value - closing.value) * props.c.initial_margin)
const fee = computed(() => Math.ceil(lots.value * (px.value > 0 ? px.value : props.c.last_price) * props.c.tick_value * (session.rules?.fees.commission ?? 0.00025)))

// "平均到 X 赚 Y": X starts a tenth past the price on the side's way, and drags across ±40%
const finalAt = ref(0)
watch([side, () => props.c.id], () => (finalAt.value = Math.round(p.value * (side.value === 'buy' ? 1.1 : 0.9))), { immediate: true })
const outcome = computed(() => {
  const entry = px.value > 0 ? px.value / 100 : p.value
  return (side.value === 'buy' ? finalAt.value - entry : entry - finalAt.value) * lots.value * perPoint.value
})

const sideLabel = computed(() => {
  if (closing.value === lots.value) return '平仓'
  return side.value === 'buy' ? '押更热' : '押更冷'
})
const mineHere = computed(() => (fut.mine?.orders ?? []).filter((o) => o.contract_id === props.c.id && o.status === 'open'))
const steps = computed(() => {
  const f = session.me?.futures_steps
  if (!f) return []
  return [
    { k: 'credit', to: '/me', label: '开通信用账户', ok: f.credit_open },
    { k: 'risk', to: '/assessment/risk', label: `风险测评 ${f.min_level} 及以上`, ok: f.risk_ok },
    { k: 'quiz', to: '/assessment/futures-quiz', label: '热度期货知识测试', ok: f.quiz_passed },
    { k: 'sign', to: '/legal/futures_disclosure?sign=1', label: '签署《热度期货风险揭示书》', ok: f.disclosure_signed },
  ]
})
/** the steps are done, or this order only closes */
const allowed = computed(() => steps.value.every((x) => x.ok) || (closing.value === lots.value && lots.value > 0))
const closed = computed(() => (session.status?.phase ?? 'closed') === 'closed' || props.c.status !== 'listed')
const problem = computed(() => {
  if (!(px.value > 0)) return '价格需大于 0，最多两位小数'
  const avail = fut.mine?.available_margin
  if (avail != null && margin.value + fee.value > avail) return `可用保证金 ${money(avail)}，不够这 ${fmtQty(lots.value)} 手`
  return ''
})

const confirming = ref(false)
const confirmTitle = computed(() => `${sideLabel.value} ${fmtQty(lots.value)} 手《${props.c.stock.title}》第 ${props.c.period} 期？`)
const confirmText = computed(() => {
  const at = num(px.value / 100)
  const rule = closing.value === lots.value
    ? ''
    : `平均热度比 ${at} 每${side.value === 'buy' ? '高' : '低'} 1 点赚 ${money(perPoint.value * lots.value, 0)} 星辉，每${side.value === 'buy' ? '低' : '高'} 1 点亏同样多。`
  return rule + (crosses.value ? `按 ${at} 以内的价格立即成交。` : `挂单等到 ${at}，当天有效。`)
})

function submit() {
  if (busy.value || problem.value) return
  if (prefs.confirmOrders) {
    confirming.value = true
    markEditing(editKey, true)
  } else place()
}
async function place() {
  confirming.value = false
  markEditing(editKey, false)
  busy.value = true
  msg.value = null
  try {
    const r = await fut.place({ contract_id: props.c.id, side: side.value, quantity: lots.value, price: px.value })
    const avg = r.average_price != null ? `，均价 ${num(r.average_price / 100)}` : ''
    msg.value = { kind: 'ok', text: r.filled === lots.value ? `已成交 ${fmtQty(r.filled)} 手${avg}` : r.filled > 0 ? `已成交 ${fmtQty(r.filled)} 手${avg}，其余挂单` : '已挂单，当天有效' }
    emit('placed')
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法提交委托') }
  } finally {
    busy.value = false
  }
}
async function cancel(id: number) {
  try {
    await fut.cancel(id)
    msg.value = { kind: 'ok', text: '已撤单' }
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法撤单') }
  }
}
</script>

<template>
  <form class="of" @submit.prevent="submit">
    <div class="sides" role="group" aria-label="方向">
      <button type="button" class="b" :aria-pressed="side === 'buy'" @click="side = 'buy'">
        <b>押更热</b><span class="num">高于 {{ num(p) }} 赚</span>
      </button>
      <button type="button" class="s" :aria-pressed="side === 'sell'" @click="side = 'sell'">
        <b>押更冷</b><span class="num">低于 {{ num(p) }} 赚</span>
      </button>
    </div>

    <label class="row">价格
      <span class="pricebox">
        <input class="tinput num" inputmode="decimal" v-model="price" autocomplete="off" aria-label="价格" />
        <button type="button" class="tbtn counter" :class="side" @click="useCounter">对手价</button>
      </span>
    </label>
    <div class="row">数量
      <span class="stepper">
        <button type="button" class="tbtn plain" aria-label="减少" @click="lots = Math.max(1, lots - 1)">−</button>
        <span class="num lots">{{ lots }} 手</span>
        <button type="button" class="tbtn plain" aria-label="增加" @click="lots++">+</button>
      </span>
    </div>

    <label v-if="closing < lots" class="whatif">
      <span>平均到</span>
      <span class="num at">{{ finalAt }}</span>
      <span>{{ outcome >= 0 ? '赚' : '亏' }}</span>
      <span class="num out" :class="outcome >= 0 ? 'tup' : 'tdown'">{{ money(Math.abs(outcome), 0) }}</span>
      <span>星辉</span>
      <input type="range" aria-label="假设的最终平均热度" :min="Math.floor(p * 0.6)" :max="Math.ceil(p * 1.4)" step="1" v-model.number="finalAt" />
    </label>

    <div class="kv"><span>初始保证金</span><span class="num v">{{ money(margin) }}</span></div>
    <div class="kv"><span>手续费</span><span class="num v">{{ money(fee) }}</span></div>
    <div v-if="fut.mine" class="kv"><span>可用保证金</span><span class="num v">{{ money(fut.mine.available_margin) }}</span></div>

    <p v-if="problem && allowed" class="note err" role="alert">{{ problem }}</p>
    <RouterLink v-if="!session.me" :to="{ name: 'login', query: { next: route.fullPath } }" class="tbtn buy cta">登录后交易</RouterLink>
    <div v-else-if="!allowed" class="steps">
      <p class="note">交易热度期货前完成：</p>
      <component :is="x.ok ? 'span' : RouterLink" v-for="x in steps" :key="x.k" :to="x.ok ? undefined : x.to" class="step" :class="{ ok: x.ok }">
        <span class="mk">{{ x.ok ? '✓' : '' }}</span><span>{{ x.label }}</span><span v-if="!x.ok" class="go">›</span>
      </component>
    </div>
    <button v-else type="submit" class="tbtn cta" :class="side === 'buy' ? 'buy' : 'sell'" :disabled="busy || !!problem || closed">
      {{ busy ? '提交中…' : `${sideLabel} ${lots} 手` }}
    </button>
    <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">{{ msg.text }}</p>

    <div v-if="mineHere.length" class="mine">
      <div class="minehead">未成交 · 当天有效</div>
      <div v-for="o in mineHere" :key="o.id" class="ord num">
        <span :class="o.side === 'buy' ? 'tup' : 'tdown'" class="sans">{{ o.side === 'buy' ? '热' : '冷' }}</span>
        <span>{{ o.price != null ? num(o.price / 100) : '市价' }}</span>
        <span>{{ o.filled }}/{{ o.quantity }}</span>
        <button type="button" class="cx" @click="cancel(o.id)">撤单</button>
      </div>
    </div>

    <Modal :open="confirming" :title="confirmTitle" :edit-key="`${editKey}-confirm`" :busy="busy" @close="confirming = false">
      <p>{{ confirmText }}</p>
      <template #actions>
        <button type="button" class="btn" :class="side === 'buy' ? 'cf-buy' : 'cf-sell'" :disabled="busy" @click="place">{{ sideLabel }}</button>
        <button type="button" class="btn ghost" :disabled="busy" @click="confirming = false">返回</button>
      </template>
    </Modal>
  </form>
</template>

<style scoped>
.of { display: flex; flex-direction: column; gap: 10px; color: var(--t-ink); }
.sides { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; }
.sides button { height: 52px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 1px; background: transparent; border: 1px solid currentColor; }
.sides b { font-size: 15px; }
.sides span { font-size: 11px; }
.sides .b { color: var(--t-up); }
.sides .s { color: var(--t-down); }
.sides .b[aria-pressed="true"] { background: var(--t-up); border-color: var(--t-up); color: var(--t-bg); }
.sides .s[aria-pressed="true"] { background: var(--t-down); border-color: var(--t-down); color: var(--t-bg); }
.row { display: grid; grid-template-columns: 48px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; color: var(--t-ink-2); }
.tinput { height: 36px; font-size: 16px; }
.pricebox { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px; min-width: 0; }
.pricebox .tinput { min-width: 0; width: 100%; }
.counter { height: 36px; padding: 0 12px; font-size: 13px; font-weight: 700; border: 1px solid currentColor; background: transparent; }
.counter.buy { color: var(--t-up); }
.counter.sell { color: var(--t-down); }
.stepper { display: grid; grid-template-columns: 40px minmax(0, 1fr) 40px; align-items: center; border: 1px solid var(--t-field-border); height: 36px; }
.stepper .tbtn { height: 34px; border: 0; font-size: 18px; }
.lots { text-align: center; font-size: 15px; color: var(--t-ink); }
.whatif { position: relative; display: flex; align-items: baseline; gap: 6px; font-size: 13px; color: var(--t-ink-2); border-top: 1px solid var(--t-line); padding-top: 10px; cursor: ew-resize; }
.whatif .at { font-size: 17px; color: var(--t-ink); border-bottom: 1px dashed var(--t-ink-2); }
.whatif .out { font-size: 17px; }
.whatif input { position: absolute; inset: 0; width: 100%; height: 100%; margin: 0; opacity: 0; cursor: ew-resize; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.cta { height: 42px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.err { color: var(--t-danger); }
.note.ok { color: var(--t-ink); }
.steps { display: flex; flex-direction: column; border-top: 1px solid var(--t-line); padding-top: 8px; }
.step { display: grid; grid-template-columns: 18px minmax(0, 1fr) auto; align-items: center; gap: 6px; padding: 9px 0; font-size: 13px; color: var(--t-ink); border-bottom: 1px solid var(--t-line); }
.step.ok { color: var(--t-ink-2); }
.step .mk { color: var(--t-up); }
.step .go { color: var(--t-ink-2); }
.mine { border-top: 1px solid var(--t-line); padding-top: 8px; display: flex; flex-direction: column; gap: 4px; }
.minehead { font-size: 12px; color: var(--t-ink-2); }
.ord { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto auto; gap: 8px; align-items: center; font-size: 12px; }
.sans { font-family: var(--font); }
.cx { border: 0; background: none; color: var(--t-ink-3); font-size: 12px; text-decoration: underline; text-underline-offset: 3px; padding: 4px 0; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
.btn.cf-buy { background: var(--up); color: #fff; }
.btn.cf-sell { background: var(--down); color: #fff; }
</style>
