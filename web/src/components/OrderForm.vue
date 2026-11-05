<script setup lang="ts">
// Order entry on the terminal surface: side, limit / market, price, quantity, cost, submit; plus the
// player's open orders in this stock (撤单).
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, ApiError, type PlaceResult, type StockDetail } from '../api'
import { hm, money, qty as fmtQty, sessionText } from '../format'
import { useAccount } from '../stores/account'
import { useSession } from '../stores/session'

const props = defineProps<{ stock: StockDetail; pick: { price: number; n: number } | null; initialSide?: 'buy' | 'sell' }>()
const emit = defineEmits<{ placed: [] }>()
const session = useSession()
const account = useAccount()
const route = useRoute()

const side = ref<'buy' | 'sell'>(props.initialSide ?? 'buy')
const kind = ref<'limit' | 'market'>('limit')
const price = ref(props.stock.quote.price.toFixed(2))
const qty = ref('')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
const cancelling = ref<number | null>(null)

watch(() => props.initialSide, (s) => s && (side.value = s))
watch(() => props.pick, (p) => {
  if (p) {
    price.value = p.price.toFixed(2)
    kind.value = 'limit'
  }
})
watch(() => props.stock.id, () => {
  price.value = props.stock.quote.price.toFixed(2)
  qty.value = ''
})

const st = computed(() => session.status)
const phase = computed(() => st.value?.phase ?? 'closed')
const auction = computed(() => phase.value === 'auction')
watch(auction, (a) => { if (a) kind.value = 'limit' }, { immediate: true })
const commissionRate = computed(() => st.value?.rules.commission ?? 0.00025)
const stampRate = computed(() => st.value?.rules.stamp_duty ?? 0.0005)
const vcm = computed(() => (props.stock.vcm?.active ? props.stock.vcm : null))

const bestAsk = computed(() => props.stock.book.asks[0]?.price ?? null)
const bestBid = computed(() => props.stock.book.bids[0]?.price ?? null)
const px = computed(() => {
  if (kind.value === 'market') return (side.value === 'buy' ? bestAsk.value : bestBid.value) ?? props.stock.quote.price
  const v = Number(price.value)
  return Number.isFinite(v) ? v : NaN
})
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v > 0 ? v : 0
})
const cash = computed(() => props.stock.available_cash ?? 0)
const sellable = computed(() => props.stock.position?.available ?? 0)
const worstAsk = computed(() => props.stock.book.asks.reduce((m, a) => Math.max(m, a.price), bestAsk.value ?? props.stock.quote.price))
const maxBuy = computed(() => {
  const p = kind.value === 'market' ? worstAsk.value : px.value
  if (!(p > 0)) return 0
  return Math.max(0, Math.floor(cash.value / (p * (1 + commissionRate.value) + 0.01)))
})
const notional = computed(() => (n.value && px.value > 0 ? px.value * n.value : 0))
const commission = computed(() => Math.ceil(notional.value * commissionRate.value * 100) / 100)
const stamp = computed(() => (side.value === 'sell' ? Math.ceil(notional.value * stampRate.value * 100) / 100 : 0))
const total = computed(() => (side.value === 'buy' ? notional.value + commission.value : notional.value - commission.value - stamp.value))

const problem = computed(() => {
  if (!n.value) return qty.value ? '数量需为正整数' : ''
  if (kind.value === 'limit' && (!(px.value > 0) || !/^\d+(\.\d{1,2})?$/.test(price.value))) return '价格需大于 0，最多两位小数'
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
    const r = await api.post<PlaceResult>('/api/orders', {
      company_id: props.stock.id, side: side.value, kind: kind.value, qty: n.value,
      price: kind.value === 'limit' ? Number(price.value).toFixed(2) : null,
    })
    msg.value = describe(r, n.value)
    if (r.filled > 0 || r.status === 'open') qty.value = ''
    emit('placed')
  } catch (e) {
    msg.value = { kind: 'error', text: e instanceof ApiError ? e.message : '无法提交委托' }
  } finally {
    busy.value = false
  }
}

function describe(r: PlaceResult, asked: number): { kind: 'ok' | 'error'; text: string } {
  const avg = r.avg_price != null ? `，均价 ${money(r.avg_price)}` : ''
  const rest = asked - r.filled
  if (auction.value && r.status === 'open' && r.filled === 0) return { kind: 'ok', text: `已提交委托，${st.value?.open ?? '09:00'} 集合竞价统一成交` }
  if (r.status === 'filled') return { kind: 'ok', text: `已成交 ${fmtQty(r.filled)} 股${avg}` }
  if (r.status === 'cancelled') {
    if (r.filled === 0) return { kind: 'error', text: '没有对手方委托，市价委托已撤销' }
    return { kind: 'ok', text: `已成交 ${fmtQty(r.filled)} 股${avg}。其余 ${fmtQty(rest)} 股没有对手方，已撤销` }
  }
  if (r.filled > 0) return { kind: 'ok', text: `已成交 ${fmtQty(r.filled)} 股${avg}。其余 ${fmtQty(rest)} 股等待成交` }
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
    msg.value = { kind: 'error', text: e instanceof ApiError ? e.message : '无法撤单' }
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
    <p v-if="auction" class="note">集合竞价时段只接受限价委托，{{ st?.open ?? '09:00' }} 统一成交</p>
    <p v-if="vcm" class="note warn">冷静期至 {{ hm(vcm.until) }}，只在 <span class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</span> 之间成交</p>

    <label v-if="kind === 'limit'" class="row">价格
      <input class="tinput num" inputmode="decimal" v-model="price" autocomplete="off" aria-label="价格" />
    </label>
    <p v-else class="note">市价委托按对手方价格成交，成交价可能偏离现价</p>
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
      <p v-if="phase === 'closed'" class="note">休市中。集合竞价 {{ st?.auction_start ?? '08:45' }}–{{ st?.open ?? '09:00' }}，连续交易 {{ sessionText(st?.open, st?.close) }}</p>
    </template>
    <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">{{ msg.text }}</p>

    <div v-if="stock.my_orders?.length" class="mine">
      <div v-for="o in stock.my_orders" :key="o.id" class="ord num">
        <span :class="o.side === 'buy' ? 'tup' : 'tdown'" class="sans">{{ o.side === 'buy' ? '买' : '卖' }}</span>
        <span>{{ o.price != null ? money(o.price) : '市价' }}</span>
        <span>{{ fmtQty(o.filled) }}/{{ fmtQty(o.qty) }}</span>
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
.fracs { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px; }
.fracs .tbtn { height: 28px; font-size: 12px; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.cta { height: 42px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.warn { color: #F2B45A; }
.note.err { color: var(--t-up); }
.note.ok { color: var(--t-ink); }
.mine { border-top: 1px solid var(--t-line); padding-top: 8px; display: flex; flex-direction: column; gap: 4px; }
.ord { display: grid; grid-template-columns: 1.5em minmax(0, 1fr) auto auto; gap: 8px; align-items: center; font-size: 12px; }
.sans { font-family: var(--font); }
.cx { border: 0; background: none; color: var(--t-ink-3); font-size: 12px; text-decoration: underline; text-underline-offset: 3px; padding: 4px 0; }
.tup { color: var(--t-up); }
.tdown { color: var(--t-down); }
</style>
