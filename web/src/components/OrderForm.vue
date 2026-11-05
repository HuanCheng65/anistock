<script setup lang="ts">
// Order entry on the terminal surface: side, limit / market, price, quantity, cost, submit; plus the
// player's open orders in this stock (撤单). 对手价 fills the best opposite price, and the line beside it
// estimates what filling the quantity now costs against the mid price, walking the visible book.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, call, errorText, type PlaceOrderResponse, type StockDetail } from '../api'
import { centsText, hm, money, qty as fmtQty, sessionText, toCents } from '../format'
import { useAccount } from '../stores/account'
import { useSession } from '../stores/session'
import type { Book } from '../stores/stock'
import { markEditing } from '../version'

const props = defineProps<{ stock: StockDetail; book: Book; pick: { price: number; n: number } | null; initialSide?: 'buy' | 'sell' }>()
const emit = defineEmits<{ placed: [] }>()
const session = useSession()
const account = useAccount()
const route = useRoute()

const side = ref<'buy' | 'sell'>(props.initialSide ?? 'buy')
const kind = ref<'limit' | 'market'>('limit')
const price = ref(centsText(props.stock.quote.price))
const qty = ref('')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
const cancelling = ref<number | null>(null)
const editKey = `order-${Math.random()}`
watch(qty, (v) => markEditing(editKey, v.trim() !== ''))
onBeforeUnmount(() => markEditing(editKey, false))

watch(() => props.initialSide, (s) => s && (side.value = s))
watch(() => props.pick, (p) => {
  if (p) {
    price.value = centsText(p.price)
    kind.value = 'limit'
  }
})
watch(() => props.stock.stock.id, () => {
  price.value = centsText(props.stock.quote.price)
  qty.value = ''
})

const st = computed(() => session.status)
const rules = computed(() => session.rules)
const phase = computed(() => st.value?.phase ?? 'closed')
const auction = computed(() => phase.value === 'auction')
watch(auction, (a) => { if (a) kind.value = 'limit' }, { immediate: true })
const commissionRate = computed(() => rules.value?.fees.commission ?? 0.00025)
const stampRate = computed(() => rules.value?.fees.stamp_duty ?? 0.0005)
const vcm = computed(() => (props.stock.vcm.active ? props.stock.vcm : null))
const sess = computed(() => rules.value?.session ?? { auction_start: '08:45', open: '09:00', close: '04:00' })

const bestAsk = computed(() => props.book.asks[0]?.price ?? null)
const bestBid = computed(() => props.book.bids[0]?.price ?? null)
/** price in cents (NaN when the text is not a valid price) */
const px = computed(() => {
  if (kind.value === 'market') return (side.value === 'buy' ? bestAsk.value : bestBid.value) ?? props.stock.quote.price
  return toCents(price.value) ?? NaN
})
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v > 0 ? v : 0
})
const cash = computed(() => session.me?.available_cash ?? 0)
const position = computed(() => (account.positions ?? []).find((p) => p.stock.id === props.stock.stock.id) ?? null)
const sellable = computed(() => position.value?.available_quantity ?? 0)
const myOrders = computed(() => (account.orders ?? []).filter((o) => o.stock.id === props.stock.stock.id && o.status === 'open'))
const worstAsk = computed(() => props.book.asks.reduce((m, a) => Math.max(m, a.price), bestAsk.value ?? props.stock.quote.price))
const maxBuy = computed(() => {
  const p = kind.value === 'market' ? worstAsk.value : px.value
  if (!(p > 0)) return 0
  return Math.max(0, Math.floor(cash.value / (p * (1 + commissionRate.value) + 1)))
})
const notional = computed(() => (n.value && px.value > 0 ? px.value * n.value : 0))
const commission = computed(() => (notional.value > 0 ? Math.ceil(notional.value * commissionRate.value) : 0))
const stamp = computed(() => (side.value === 'sell' && notional.value > 0 ? Math.ceil(notional.value * stampRate.value) : 0))
const total = computed(() => (side.value === 'buy' ? notional.value + commission.value : notional.value - commission.value - stamp.value))

// ---- 对手价 and the cost of filling now
const counter = computed(() => (side.value === 'buy' ? bestAsk.value : bestBid.value))
function useCounter() {
  if (counter.value == null) return
  kind.value = 'limit'
  price.value = centsText(counter.value)
}
const mid = computed(() => {
  const a = bestAsk.value, b = bestBid.value
  if (a != null && b != null) return (a + b) / 2
  return props.stock.quote.price
})
/** Filling `n` now against the visible book: the average price, and the extra paid (buy) or given up
 *  (sell) against the mid price, in cents and as a share of the mid value. null: the book is too thin. */
const impact = computed(() => {
  const want = n.value || 1
  const levels = side.value === 'buy' ? props.book.asks : props.book.bids
  let left = want
  let cost = 0
  for (const l of levels) {
    const take = Math.min(left, l.quantity)
    cost += take * l.price
    left -= take
    if (!left) break
  }
  if (left > 0 || !(mid.value > 0)) return null
  const extra = side.value === 'buy' ? cost - mid.value * want : mid.value * want - cost
  return { extra: Math.max(0, Math.round(extra)), ratio: Math.max(0, extra / (mid.value * want)) }
})
const impactText = computed(() => {
  if (!props.book.asks.length && !props.book.bids.length) return ''
  if (counter.value == null) return side.value === 'buy' ? '暂无卖出委托' : '暂无买入委托'
  const x = impact.value
  if (!x) return '盘口深度不足'
  const pctText = (x.ratio * 100).toFixed(2) + '%'
  if (!n.value) return `立即成交 · 每股约${side.value === 'buy' ? '多付' : '少收'} ${pctText}`
  return `立即成交 · 约${side.value === 'buy' ? '多付' : '少收'} ${pctText}（${money(x.extra)} 币）`
})

const problem = computed(() => {
  if (!n.value) return qty.value ? '数量需为正整数' : ''
  if (kind.value === 'limit' && !(px.value > 0)) return '价格需大于 0，最多两位小数'
  if (side.value === 'buy' && n.value > maxBuy.value) return `可用资金最多买入 ${fmtQty(maxBuy.value)} 股`
  if (side.value === 'sell' && n.value > sellable.value) return `可卖 ${fmtQty(sellable.value)} 股`
  return ''
})
const outsideBand = computed(() =>
  vcm.value && kind.value === 'limit' && px.value > 0 && vcm.value.low != null && vcm.value.high != null &&
  (px.value < vcm.value.low || px.value > vcm.value.high))

function fill(frac: number) {
  const m = side.value === 'buy' ? maxBuy.value : sellable.value
  qty.value = String(Math.max(0, Math.floor(m * frac)))
}

async function submit() {
  if (busy.value || problem.value || !n.value) return
  busy.value = true
  msg.value = null
  try {
    const r = await call(api.POST('/api/v1/me/orders', {
      body: {
        stock_id: props.stock.stock.id, side: side.value, type: kind.value, quantity: n.value,
        price: kind.value === 'limit' ? px.value : null,
      },
      headers: { 'Idempotency-Key': `${Date.now()}-${Math.random().toString(36).slice(2)}` },
    }))
    msg.value = describe(r, n.value)
    if (r.order.filled_quantity > 0 || r.order.status === 'open') qty.value = ''
    if (account.orders) account.refreshOrders().catch(() => {})
    emit('placed')
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法提交委托') }
  } finally {
    busy.value = false
  }
}

function describe(r: PlaceOrderResponse, asked: number): { kind: 'ok' | 'error'; text: string } {
  const o = r.order
  const avg = o.average_fill_price != null ? `，均价 ${money(o.average_fill_price)}` : ''
  const rest = asked - o.filled_quantity
  if (auction.value && o.status === 'open' && o.filled_quantity === 0) return { kind: 'ok', text: `已提交委托，${sess.value.open} 集合竞价统一成交` }
  if (o.status === 'filled') return { kind: 'ok', text: `已成交 ${fmtQty(o.filled_quantity)} 股${avg}` }
  if (o.status === 'cancelled') {
    if (o.filled_quantity === 0) return { kind: 'error', text: '没有对手方委托，市价委托已撤销' }
    return { kind: 'ok', text: `已成交 ${fmtQty(o.filled_quantity)} 股${avg}。其余 ${fmtQty(rest)} 股没有对手方，已撤销` }
  }
  if (o.filled_quantity > 0) return { kind: 'ok', text: `已成交 ${fmtQty(o.filled_quantity)} 股${avg}。其余 ${fmtQty(rest)} 股等待成交` }
  if (vcm.value) return { kind: 'ok', text: `已提交委托。冷静期内只在 ${money(vcm.value.low)} – ${money(vcm.value.high)} 之间成交` }
  return { kind: 'ok', text: '已提交委托' }
}

async function cancel(id: number) {
  cancelling.value = id
  msg.value = null
  try {
    await account.cancel(id)
    msg.value = { kind: 'ok', text: '已撤单' }
    emit('placed')
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法撤单') }
  } finally {
    cancelling.value = null
  }
}
</script>

<template>
  <form class="of" @submit.prevent="submit">
    <div class="sides" role="tablist" aria-label="方向">
      <button type="button" role="tab" :aria-selected="side === 'buy'" class="b" @click="side = 'buy'">买入</button>
      <button type="button" role="tab" :aria-selected="side === 'sell'" class="s" @click="side = 'sell'">卖出</button>
    </div>
    <div class="kinds" role="tablist" aria-label="委托类型">
      <button type="button" role="tab" :aria-selected="kind === 'limit'" @click="kind = 'limit'">限价</button>
      <button type="button" role="tab" :aria-selected="kind === 'market'" :disabled="auction" @click="kind = 'market'">市价</button>
    </div>
    <p v-if="auction" class="note">集合竞价时段只接受限价委托，{{ sess.open }} 统一成交</p>
    <p v-if="vcm" class="note warn">冷静期至 {{ hm(vcm.until) }}，只在 <span class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</span> 之间成交</p>

    <label v-if="kind === 'limit'" class="row">价格
      <span class="pricebox">
        <input class="tinput num" inputmode="decimal" v-model="price" autocomplete="off" aria-label="价格" />
        <button type="button" class="tbtn counter" :class="side" :disabled="counter == null" @click="useCounter">对手价</button>
      </span>
    </label>
    <p v-else class="note">市价委托按对手方价格成交，成交价可能偏离现价</p>
    <p v-if="impactText" class="note impact num">{{ impactText }}</p>
    <label class="row">数量
      <input class="tinput num" inputmode="numeric" v-model="qty" autocomplete="off" aria-label="数量"
             :placeholder="side === 'buy' ? `最多 ${fmtQty(maxBuy)}` : `可卖 ${fmtQty(sellable)}`" />
    </label>
    <div class="fracs">
      <button type="button" class="tbtn plain num" @click="fill(1 / 4)">1/4</button>
      <button type="button" class="tbtn plain num" @click="fill(1 / 3)">1/3</button>
      <button type="button" class="tbtn plain num" @click="fill(1 / 2)">1/2</button>
      <button type="button" class="tbtn plain" @click="fill(1)">全部</button>
    </div>
    <div class="kv"><span>{{ side === 'buy' ? '可用资金' : '可卖' }}</span><span class="num v">{{ side === 'buy' ? money(cash) : fmtQty(sellable) }}</span></div>
    <div class="kv" :title="`含佣金${side === 'sell' ? '和印花税' : ''}`"><span>{{ side === 'buy' ? '预计花费' : '预计到账' }}</span><span class="num v">{{ n ? money(total) : '–' }}</span></div>

    <p v-if="problem" class="note err" role="alert">{{ problem }}</p>
    <p v-else-if="outsideBand" class="note warn">价格在冷静期区间外，冷静期结束前不会成交</p>

    <RouterLink v-if="!session.me" :to="{ name: 'login', query: { next: route.fullPath } }" class="tbtn buy cta">登录后交易</RouterLink>
    <template v-else>
      <button type="submit" class="tbtn cta" :class="side === 'buy' ? 'buy' : 'sell'" :disabled="busy || !!problem || !n || phase === 'closed'">
        {{ busy ? '提交中…' : side === 'buy' ? '买入' : '卖出' }}
      </button>
      <p v-if="phase === 'closed'" class="note">休市中。集合竞价 {{ sess.auction_start }}–{{ sess.open }}，连续交易 {{ sessionText(sess.open, sess.close) }}</p>
    </template>
    <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">{{ msg.text }}</p>

    <div v-if="myOrders.length" class="mine">
      <div v-for="o in myOrders" :key="o.id" class="ord num">
        <span :class="o.side === 'buy' ? 'tup' : 'tdown'" class="sans">{{ o.side === 'buy' ? '买' : '卖' }}</span>
        <span>{{ o.price != null ? money(o.price) : '市价' }}</span>
        <span>{{ fmtQty(o.filled_quantity) }}/{{ fmtQty(o.quantity) }}</span>
        <button type="button" class="cx" :disabled="cancelling === o.id" @click="cancel(o.id)">{{ cancelling === o.id ? '撤单中' : '撤单' }}</button>
      </div>
    </div>
  </form>
</template>

<style scoped>
.of { display: flex; flex-direction: column; gap: 10px; color: var(--t-ink); }
.sides { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); border: 1px solid var(--t-field-border); }
.sides button { height: 36px; border: 0; font-size: 14px; font-weight: 700; background: transparent; color: var(--t-ink-2); }
.sides .b[aria-selected="true"] { background: var(--t-up); color: var(--t-bg); }
.sides .s[aria-selected="true"] { background: var(--t-down); color: var(--t-bg); }
.kinds { display: flex; gap: 16px; font-size: 13px; }
.kinds button { border: 0; background: none; padding: 2px 0; color: var(--t-ink-2); }
.kinds button[aria-selected="true"] { color: var(--t-ink); font-weight: 700; }
.kinds button:disabled { opacity: 0.4; }
.row { display: grid; grid-template-columns: 48px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; color: var(--t-ink-2); }
.tinput { height: 36px; font-size: 16px; }
.pricebox { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px; min-width: 0; }
.pricebox .tinput { min-width: 0; width: 100%; }
.counter { height: 36px; padding: 0 12px; font-size: 13px; font-weight: 700; white-space: nowrap; border: 1px solid currentColor; }
.counter.buy { color: var(--t-up); border-color: var(--t-up); background: transparent; }
.counter.sell { color: var(--t-down); border-color: var(--t-down); background: transparent; }
.counter:disabled { opacity: 0.4; }
.impact { color: var(--t-ink); font-family: var(--font); margin-top: -4px; }
.fracs { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px; }
.fracs .tbtn { height: 28px; font-size: 12px; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.cta { height: 42px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.warn { color: #F2B45A; }
.note.err { color: var(--t-danger); }
.note.ok { color: var(--t-ink); }
.mine { border-top: 1px solid var(--t-line); padding-top: 8px; display: flex; flex-direction: column; gap: 4px; }
.ord { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto auto; gap: 8px; align-items: center; font-size: 12px; }
.sans { font-family: var(--font); }
.cx { border: 0; background: none; color: var(--t-ink-3); font-size: 12px; text-decoration: underline; text-underline-offset: 3px; padding: 4px 0; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
</style>
