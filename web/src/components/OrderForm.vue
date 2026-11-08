<script setup lang="ts">
// Order entry on the terminal surface: side, price, quantity, cost, submit; plus the player's open orders in this
// stock (撤单). Every order is a limit order: the price starts at 对手价 (the best opposite price), so the default
// fills now, and the line under it says which: filling now (and what that costs against the mid price, walking
// the visible book), or waiting until the other side comes to it, good until the daily close. Market orders stay
// in the API; here a price that crosses the book is the same thing, with a limit.
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, ApiError, call, errorText, type PlaceOrderResponse, type StockDetail } from '../api'
import { centsText, hm, money, qty as fmtQty, sessionText, toCents } from '../format'
import { useAccount } from '../stores/account'
import { usePrefs } from '../stores/prefs'
import { MODE_LABEL, useCredit } from '../stores/credit'
import { useSession } from '../stores/session'
import type { Book } from '../stores/stock'
import { markEditing } from '../version'
import Modal from './Modal.vue'

const props = defineProps<{ stock: StockDetail; book: Book; pick: { price: number; n: number } | null; initialSide?: 'buy' | 'sell' }>()
const emit = defineEmits<{ placed: [] }>()
const session = useSession()
const account = useAccount()
const credit = useCredit()
const route = useRoute()
const prefs = usePrefs()

const side = ref<'buy' | 'sell'>(props.initialSide ?? 'buy')
const price = ref('')
const qty = ref('')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string; sign?: boolean } | null>(null)
const cancelling = ref<number | null>(null)
const editKey = `order-${Math.random()}`
watch(qty, (v) => markEditing(editKey, v.trim() !== ''))
onBeforeUnmount(() => markEditing(editKey, false))

watch(() => props.initialSide, (s) => s && (side.value = s))
watch(() => props.pick, (p) => {
  if (p) price.value = centsText(p.price)
})


const st = computed(() => session.status)
const rules = computed(() => session.rules)
const phase = computed(() => st.value?.phase ?? 'closed')
const auction = computed(() => phase.value === 'auction')
const commissionRate = computed(() => rules.value?.fees.commission ?? 0.00025)
const stampRate = computed(() => rules.value?.fees.stamp_duty ?? 0.0005)
const vcm = computed(() => (props.stock.vcm.active ? props.stock.vcm : null))
const sess = computed(() => rules.value?.session ?? { auction_start: '08:45', open: '09:00', close: '04:00', daily_close: '06:30' })

const bestAsk = computed(() => props.book.asks[0]?.price ?? null)
const bestBid = computed(() => props.book.bids[0]?.price ?? null)
/** price in cents (NaN when the text is not a valid price) */
const px = computed(() => toCents(price.value) ?? NaN)
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v > 0 ? v : 0
})
const cash = computed(() => session.me?.available_cash ?? 0)
const position = computed(() => (account.positions ?? []).find((p) => p.stock.id === props.stock.stock.id) ?? null)
const sellable = computed(() => position.value?.available_quantity ?? 0)
const myOrders = computed(() => (account.orders ?? []).filter((o) => o.stock.id === props.stock.stock.id && o.status === 'open'))

// ---- 融资融券: the trading mode on each side, shown once credit is open (docs/design/margin.md)
type Mode = 'plain' | 'margin_buy' | 'short_sell' | 'sell_repay' | 'buy_return'
const mode = ref<Mode>('plain')
const sm = computed(() => credit.margins[props.stock.stock.id] ?? null)
const ca = computed(() => credit.account)
watch([() => credit.usable, () => props.stock.stock.id], ([u, id]) => {
  if (!u) return
  credit.loadMargin(id).catch(() => {})
  if (!credit.account) credit.refresh().catch(() => {})
}, { immediate: true })
const short = computed(() => ca.value?.shorts.find((s) => s.stock.id === props.stock.stock.id) ?? null)
const owedShares = computed(() => short.value?.quantity ?? 0)
const owedCash = computed(() => (ca.value ? ca.value.debt + ca.value.interest : 0))
const modes = computed<Mode[]>(() => {
  if (!credit.usable || !ca.value) return ['plain']
  const open = !!sm.value?.eligible && ca.value.status === 'normal'
  if (side.value === 'buy') {
    if (owedShares.value) return ['buy_return']
    return open ? ['plain', 'margin_buy'] : ['plain']
  }
  const out: Mode[] = ['plain']
  if (open && !position.value?.quantity) out.push('short_sell')
  if (owedCash.value > 0 && sellable.value > 0) out.push('sell_repay')
  return out
})
watch(modes, (ms) => { if (!ms.includes(mode.value)) mode.value = ms[0] }, { immediate: true })
const creditMode = computed(() => (mode.value === 'plain' ? null : mode.value))
const maxBuy = computed(() => {
  const p = px.value
  if (!(p > 0)) return 0
  return Math.max(0, Math.floor(cash.value / (p * (1 + commissionRate.value) + 1)))
})
const last = computed(() => props.stock.quote.price)
/** the most this mode allows at the entered price */
const maxQty = computed(() => {
  const p = px.value
  const per = p * (1 + commissionRate.value) + 1
  const c = ca.value
  switch (mode.value) {
    case 'margin_buy': {
      if (!(p > 0) || !c || !sm.value) return 0
      // each share: 保证金比例 × the loan, plus the loan over the share's value (commission) counted in full
      const byMargin = Math.floor(c.available_margin / (per * c.margin_ratio + Math.max(0, per - last.value)))
      return Math.max(0, Math.min(byMargin, Math.floor(sm.value.financing_room / per)))
    }
    case 'short_sell': {
      if (!(p > 0) || !c || !sm.value) return 0
      return Math.max(0, Math.min(sm.value.lendable, Math.floor(c.available_margin / (c.margin_ratio * Math.max(p, last.value)))))
    }
    case 'buy_return':
      if (!(p > 0)) return 0
      return Math.max(0, Math.min(owedShares.value, Math.floor((cash.value + (short.value?.proceeds ?? 0)) / per)))
    case 'sell_repay':
      return sellable.value
    default:
      return side.value === 'buy' ? maxBuy.value : sellable.value
  }
})
const notional = computed(() => (n.value && px.value > 0 ? px.value * n.value : 0))
const commission = computed(() => (notional.value > 0 ? Math.ceil(notional.value * commissionRate.value) : 0))
const stamp = computed(() => (side.value === 'sell' && notional.value > 0 ? Math.ceil(notional.value * stampRate.value) : 0))
const total = computed(() => (side.value === 'buy' ? notional.value + commission.value : notional.value - commission.value - stamp.value))

// ---- 对手价 and the cost of filling now
const counter = computed(() => (side.value === 'buy' ? bestAsk.value : bestBid.value))
function useCounter() {
  price.value = centsText(counter.value ?? props.stock.quote.price)
}
// the price starts at 对手价 (the last price on an empty side), and again on a new stock or side; a price the
// player typed or picked from the book stays until then
watch([() => props.stock.stock.id, side], ([id], [was]) => {
  useCounter()
  if (id !== was) qty.value = ''
}, { immediate: true })
/** whether the entered price meets the other side now (in the auction: whether it is in the cross) */
const crosses = computed(() => {
  const c = counter.value
  if (c == null || !(px.value > 0)) return false
  return side.value === 'buy' ? px.value >= c : px.value <= c
})
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
/** the line under the price: fills now (and its cost), or waits until the other side comes, until the daily close */
const fillText = computed(() => {
  if (!(px.value > 0)) return ''
  const until = `${sess.value.daily_close} 前有效`
  if (auction.value) return `${sess.value.open} 集合竞价统一成交，${until}`
  if (!crosses.value) {
    if (counter.value == null) return `${side.value === 'buy' ? '暂无卖出委托' : '暂无买入委托'}，挂单等待，${until}`
    return `${side.value === 'buy' ? '卖价降到' : '买价升到'} ${money(px.value)} 才成交，${until}`
  }
  const x = impact.value
  if (!x) return '立即成交一部分，其余挂单等待'
  if (!n.value || x.extra === 0) return '立即成交'
  return `立即成交，比中间价约${side.value === 'buy' ? '多付' : '少收'} ${money(x.extra)} 星辉`
})

const problem = computed(() => {
  if (!n.value) return qty.value ? '数量需为正整数' : ''
  if (!(px.value > 0)) return '价格需大于 0，最多两位小数'
  if (mode.value === 'short_sell' && px.value < last.value) return `融券卖出价格不能低于最新价 ${money(last.value)}`
  if (creditMode.value && n.value > maxQty.value) return `最多${MODE_LABEL[mode.value]} ${fmtQty(maxQty.value)} 股`
  if (creditMode.value) return ''
  if (side.value === 'buy' && n.value > maxBuy.value) return `可用资金最多买入 ${fmtQty(maxBuy.value)} 股`
  if (side.value === 'sell' && n.value > sellable.value) return `可卖 ${fmtQty(sellable.value)} 股`
  return ''
})
const outsideBand = computed(() =>
  vcm.value && px.value > 0 && vcm.value.low != null && vcm.value.high != null &&
  (px.value < vcm.value.low || px.value > vcm.value.high))

function fill(frac: number) {
  qty.value = String(Math.max(0, Math.floor(maxQty.value * frac)))
}
const modeFact = computed<{ k: string; v: string } | null>(() => {
  switch (mode.value) {
    case 'margin_buy': return { k: '可用保证金', v: money(ca.value?.available_margin ?? 0) }
    case 'short_sell': return { k: '可融券', v: fmtQty(sm.value?.lendable ?? 0) }
    case 'buy_return': return { k: '欠券', v: fmtQty(owedShares.value) }
    case 'sell_repay': return { k: '融资欠款', v: money(owedCash.value) }
    default: return null
  }
})
const submitText = computed(() => (creditMode.value ? MODE_LABEL[mode.value] : side.value === 'buy' ? '买入' : '卖出'))

// 下单前确认 (settings): the press opens a dialog with the order; its button sends
const confirming = ref(false)
const confirmTitle = computed(() => `${submitText.value} ${fmtQty(n.value)} 股《${props.stock.stock.title}》？`)
const confirmText = computed(() => (crosses.value && !auction.value
  ? `按 ${money(px.value)} 以内的价格立即成交，${side.value === 'buy' ? '约花' : '约到账'} ${money(total.value)} 星辉。`
  : `挂单等到 ${money(px.value)}，${sess.value.daily_close} 前没有成交会自动撤销。`))

function submit() {
  if (busy.value || problem.value || !n.value) return
  if (prefs.confirmOrders) confirming.value = true
  else place()
}

async function place() {
  if (busy.value || problem.value || !n.value) return
  busy.value = true
  msg.value = null
  try {
    const r = await call(api.POST('/api/v1/me/orders', {
      body: {
        stock_id: props.stock.stock.id, side: side.value, type: 'limit', quantity: n.value,
        price: px.value,
        ...(creditMode.value ? { credit: creditMode.value } : {}),
      },
      headers: { 'Idempotency-Key': `${Date.now()}-${Math.random().toString(36).slice(2)}` },
    }))
    msg.value = describe(r, n.value)
    if (r.order.filled_quantity > 0 || r.order.status === 'open') qty.value = ''
    if (account.orders) account.refreshOrders().catch(() => {})
    if (creditMode.value || credit.account?.opened) {
      credit.refresh().catch(() => {})
      credit.loadMargin(props.stock.stock.id).catch(() => {})
    }
    emit('placed')
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法提交委托'), sign: e instanceof ApiError && e.code === 'agreement_required' }
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
    if (o.filled_quantity === 0) return { kind: 'error', text: '没有成交，委托已撤销' }
    return { kind: 'ok', text: `已成交 ${fmtQty(o.filled_quantity)} 股${avg}。其余 ${fmtQty(rest)} 股已撤销` }
  }
  if (o.filled_quantity > 0) return { kind: 'ok', text: `已成交 ${fmtQty(o.filled_quantity)} 股${avg}。其余 ${fmtQty(rest)} 股等待成交` }
  if (vcm.value) return { kind: 'ok', text: `已提交委托。冷静期内只在 ${money(vcm.value.low)} – ${money(vcm.value.high)} 之间成交` }
  return { kind: 'ok', text: `已挂单，${sess.value.daily_close} 前有效` }
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
    <div v-if="modes.length > 1 || modes[0] !== 'plain'" class="modes" role="tablist" aria-label="交易方式">
      <button v-for="m in modes" :key="m" type="button" role="tab" :aria-selected="mode === m" @click="mode = m">
        {{ m === 'plain' ? '普通' : MODE_LABEL[m] }}
      </button>
    </div>
    <p v-if="mode === 'short_sell'" class="note">融券卖出价格不低于最新价 <span class="num">{{ money(last) }}</span></p>
    <p v-if="vcm" class="note warn">冷静期至 {{ hm(vcm.until) }}，只在 <span class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</span> 之间成交</p>

    <label class="row">价格
      <span class="pricebox">
        <input class="tinput num" inputmode="decimal" v-model="price" autocomplete="off" aria-label="价格" />
        <button type="button" class="tbtn counter" :class="side" :disabled="counter == null" @click="useCounter">对手价</button>
      </span>
    </label>
    <p v-if="fillText" class="note impact" :class="{ wait: !crosses || auction }">{{ fillText }}</p>
    <label class="row">数量
      <input class="tinput num" inputmode="numeric" v-model="qty" autocomplete="off" aria-label="数量"
             :placeholder="side === 'sell' && !creditMode ? `可卖 ${fmtQty(sellable)}` : `最多 ${fmtQty(maxQty)}`" />
    </label>
    <div class="fracs">
      <button type="button" class="tbtn plain num" @click="fill(1 / 4)">1/4</button>
      <button type="button" class="tbtn plain num" @click="fill(1 / 2)">1/2</button>
      <button type="button" class="tbtn plain" @click="fill(1)">全部</button>
    </div>
    <div v-if="modeFact" class="kv"><span>{{ modeFact.k }}</span><span class="num v">{{ modeFact.v }}</span></div>
    <div v-else class="kv"><span>{{ side === 'buy' ? '可用资金' : '可卖' }}</span><span class="num v">{{ side === 'buy' ? money(cash) : fmtQty(sellable) }}</span></div>
    <div class="kv" :title="`含佣金${side === 'sell' ? '和印花税' : ''}`"><span>{{ side === 'buy' ? '预计花费' : '预计到账' }}</span><span class="num v">{{ n ? money(total) : '–' }}</span></div>

    <p v-if="problem" class="note err" role="alert">{{ problem }}</p>
    <p v-else-if="outsideBand" class="note warn">价格在冷静期区间外，冷静期结束前不会成交</p>

    <RouterLink v-if="!session.me" :to="{ name: 'login', query: { next: route.fullPath } }" class="tbtn buy cta">登录后交易</RouterLink>
    <template v-else>
      <button type="submit" class="tbtn cta" :class="side === 'buy' ? 'buy' : 'sell'" :disabled="busy || !!problem || !n || phase === 'closed'">
        {{ busy ? '提交中…' : submitText }}
      </button>
      <Modal :open="confirming" :title="confirmTitle" :edit-key="`${editKey}-confirm`" :busy="busy" @close="confirming = false">
        <p>{{ confirmText }}</p>
        <template #actions>
          <button type="button" class="btn" :class="side === 'buy' ? 'cf-buy' : 'cf-sell'" :disabled="busy" @click="confirming = false; place()">{{ submitText }}</button>
          <button type="button" class="btn ghost" :disabled="busy" @click="confirming = false">取消</button>
        </template>
      </Modal>
      <p v-if="phase === 'closed'" class="note">休市中。集合竞价 {{ sess.auction_start }}–{{ sess.open }}，连续交易 {{ sessionText(sess.open, sess.close) }}</p>
    </template>
    <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">
      {{ msg.text }}
      <RouterLink v-if="msg.sign" :to="{ name: 'welcome', query: { next: route.fullPath } }" class="signlink">签署</RouterLink>
    </p>

    <div v-if="myOrders.length" class="mine">
      <div class="minehead">未成交 · {{ sess.daily_close }} 前有效</div>
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
.signlink { margin-left: 8px; color: var(--t-ink); font-weight: 700; text-decoration: underline; text-underline-offset: 3px; }
.btn.cf-buy { background: var(--up); color: #fff; }
.btn.cf-sell { background: var(--down); color: #fff; }
.of { display: flex; flex-direction: column; gap: 10px; color: var(--t-ink); }
.sides { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); border: 1px solid var(--t-field-border); }
.sides button { height: 36px; border: 0; font-size: 14px; font-weight: 700; background: transparent; color: var(--t-ink-2); }
.sides .b[aria-selected="true"] { background: var(--t-up); color: var(--t-bg); }
.sides .s[aria-selected="true"] { background: var(--t-down); color: var(--t-bg); }
.modes { display: flex; gap: 6px; flex-wrap: wrap; }
.modes button { height: 28px; padding: 0 10px; border: 1px solid var(--t-field-border); background: transparent; color: var(--t-ink-2); font-size: 12px; }
.modes button[aria-selected="true"] { border-color: var(--t-ink); color: var(--t-ink); font-weight: 700; }
.row { display: grid; grid-template-columns: 48px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; color: var(--t-ink-2); }
.tinput { height: 36px; font-size: 16px; }
.pricebox { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px; min-width: 0; }
.pricebox .tinput { min-width: 0; width: 100%; }
.counter { height: 36px; padding: 0 12px; font-size: 13px; font-weight: 700; white-space: nowrap; border: 1px solid currentColor; }
.counter.buy { color: var(--t-up); border-color: var(--t-up); background: transparent; }
.counter.sell { color: var(--t-down); border-color: var(--t-down); background: transparent; }
.counter:disabled { opacity: 0.4; }
.impact { color: var(--t-ink); margin-top: -4px; padding-left: 56px; }
.impact.wait { color: #F2B45A; }
.fracs { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
.fracs .tbtn { height: 28px; font-size: 12px; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.cta { height: 42px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.warn { color: #F2B45A; }
.note.err { color: var(--t-danger); }
.note.ok { color: var(--t-ink); }
.mine { border-top: 1px solid var(--t-line); padding-top: 8px; display: flex; flex-direction: column; gap: 4px; }
.minehead { font-size: 12px; color: var(--t-ink-2); }
.ord { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto auto; gap: 8px; align-items: center; font-size: 12px; }
.sans { font-family: var(--font); }
.cx { border: 0; background: none; color: var(--t-ink-3); font-size: 12px; text-decoration: underline; text-underline-offset: 3px; padding: 4px 0; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
</style>
