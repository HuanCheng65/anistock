<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, ApiError, type PlaceResult, type StockDetail } from '../api'
import { hhmm, money } from '../format'
import { useSession } from '../stores/session'

const props = defineProps<{ stock: StockDetail; pick: { price: number; n: number } | null }>()
const emit = defineEmits<{ placed: [] }>()
const session = useSession()
const route = useRoute()

const side = ref<'buy' | 'sell'>('buy')
const kind = ref<'limit' | 'market'>('limit')
const price = ref(props.stock.quote.price.toFixed(2))
const qty = ref<string>('')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error' | 'info'; text: string } | null>(null)

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
const phase = computed(() => st.value?.phase ?? (st.value?.trading ? 'continuous' : 'closed'))
const auction = computed(() => phase.value === 'auction')
watch(auction, (a) => { if (a) kind.value = 'limit' }, { immediate: true })

const commissionRate = computed(() => st.value?.rules.commission ?? 0.00025)
const stampRate = computed(() => st.value?.rules.stamp_duty ?? 0.0005)
const vcm = computed(() => (props.stock.vcm?.active ? props.stock.vcm : null))

const bestAsk = computed(() => props.stock.book.asks[0]?.price ?? null)
const bestBid = computed(() => props.stock.book.bids[0]?.price ?? null)
const px = computed(() => {
  if (kind.value === 'market') {
    const p = side.value === 'buy' ? bestAsk.value : bestBid.value
    return p ?? props.stock.quote.price
  }
  const v = Number(price.value)
  return Number.isFinite(v) ? v : NaN
})
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v > 0 ? v : 0
})
const cash = computed(() => props.stock.available_cash ?? 0)
const sellable = computed(() => props.stock.position?.available ?? 0)

// market buys are bounded by what the cash covers at the worst visible ask
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
  if (!n.value) return qty.value ? '数量必须是正整数' : ''
  if (kind.value === 'limit' && (!(px.value > 0) || !/^\d+(\.\d{1,2})?$/.test(price.value))) return '价格最多两位小数'
  if (side.value === 'buy' && n.value > maxBuy.value) return `资金最多够买 ${maxBuy.value} 股`
  if (side.value === 'sell' && n.value > sellable.value) return `可卖 ${sellable.value} 股`
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
    msg.value = { kind: 'error', text: e instanceof ApiError ? e.message : '下单失败' }
  } finally {
    busy.value = false
  }
}

function describe(r: PlaceResult, asked: number): { kind: 'ok' | 'error' | 'info'; text: string } {
  const verb = side.value === 'buy' ? '买入' : '卖出'
  const avg = r.avg_price != null ? `，均价 ${money(r.avg_price)}` : ''
  if (auction.value && r.status === 'open' && r.filled === 0)
    return { kind: 'info', text: `已挂单，${st.value?.open ?? '09:00'} 集合竞价统一撮合。开盘前可以随时撤单` }
  if (r.status === 'filled') return { kind: 'ok', text: `已全部${verb} ${r.filled} 股${avg}` }
  if (r.status === 'cancelled') {
    if (r.filled === 0) return { kind: 'info', text: '没有可成交的对手盘，市价单已自动撤销，没有扣钱' }
    return { kind: 'ok', text: `${verb}成交 ${r.filled} 股${avg}，剩余 ${asked - r.filled} 股没有对手盘，已自动撤销` }
  }
  if (r.filled > 0) return { kind: 'ok', text: `已${verb} ${r.filled} 股${avg}，剩余 ${asked - r.filled} 股挂单中` }
  if (vcm.value) return { kind: 'info', text: `已挂单。冷静期中只在 ${money(vcm.value.low)} – ${money(vcm.value.high)} 之间成交` }
  return { kind: 'info', text: '已挂单，等待成交。可以在下方“本股委托”里撤单' }
}
</script>

<template>
  <div class="op">
    <div class="seg trade" role="group" aria-label="买卖方向">
      <button type="button" class="b" :aria-pressed="side === 'buy'" @click="side = 'buy'">买入</button>
      <button type="button" class="s" :aria-pressed="side === 'sell'" @click="side = 'sell'">卖出</button>
    </div>
    <div class="seg" role="group" aria-label="委托类型">
      <button type="button" :aria-pressed="kind === 'limit'" @click="kind = 'limit'">限价</button>
      <button type="button" :aria-pressed="kind === 'market'" :disabled="auction" @click="kind = 'market'">市价</button>
    </div>
    <p v-if="auction" class="notice small">集合竞价时段只收限价单，{{ st?.open ?? '09:00' }} 按同一个价格统一撮合。</p>
    <p v-if="vcm" class="notice small">冷静期中，至 {{ hhmm(vcm.until) }} 只能在 <b class="num">{{ money(vcm.low) }} – {{ money(vcm.high) }}</b> 之间成交，区间外的单子会挂着等。</p>

    <label class="field" v-if="kind === 'limit'">
      <span class="label">价格（币） · 昨收 <span class="num">{{ money(stock.quote.prev_close) }}</span></span>
      <input id="order-price" class="input num" inputmode="decimal" v-model="price" autocomplete="off" />
    </label>
    <p v-else class="small muted">市价单从盘口最优价开始成交，成交不了的部分自动撤销。没有涨跌停，量大时会吃到很差的价格，大单建议用限价。</p>

    <label class="field">
      <span class="label">
        数量（股）·
        <template v-if="side === 'buy'">最多可买 <span class="num">{{ maxBuy }}</span></template>
        <template v-else>可卖 <span class="num">{{ sellable }}</span></template>
      </span>
      <input id="order-qty" class="input num" inputmode="numeric" v-model="qty" placeholder="输入股数" autocomplete="off" />
    </label>
    <div class="quick">
      <button type="button" class="btn small" @click="fill(0.25)">1/4</button>
      <button type="button" class="btn small" @click="fill(0.5)">1/2</button>
      <button type="button" class="btn small" @click="fill(1)">全部</button>
    </div>

    <dl class="est small" v-if="n">
      <div><dt>{{ kind === 'market' ? '预估金额' : '金额' }}</dt><dd class="num">{{ money(notional) }}</dd></div>
      <div><dt>佣金（{{ (commissionRate * 100).toFixed(3) }}%）</dt><dd class="num">{{ money(commission) }}</dd></div>
      <div v-if="side === 'sell'"><dt>印花税（{{ (stampRate * 100).toFixed(2) }}%）</dt><dd class="num">{{ money(stamp) }}</dd></div>
      <div class="tot"><dt>{{ side === 'buy' ? '合计支出' : '预计到账' }}</dt><dd class="num">{{ money(total) }}</dd></div>
    </dl>

    <p v-if="problem" class="notice error">{{ problem }}</p>
    <p v-else-if="outsideBand" class="notice small">这个价格在冷静期区间外，冷静期结束前不会成交。</p>
    <p v-if="msg" class="notice" :class="msg.kind === 'error' ? 'error' : msg.kind === 'ok' ? 'ok' : ''" role="status">{{ msg.text }}</p>

    <template v-if="!session.me">
      <RouterLink :to="{ name: 'login', query: { next: route.fullPath } }" class="btn primary block">登录后交易</RouterLink>
    </template>
    <template v-else>
      <button type="button" class="btn block" :class="side === 'buy' ? 'buy' : 'sell'" :disabled="busy || !!problem || !n || phase === 'closed'" @click="submit">
        {{ busy ? '提交中…' : (auction ? '竞价' : '') + (side === 'buy' ? '买入' : '卖出') }}
      </button>
      <p v-if="phase === 'closed'" class="small muted center">现在休市。集合竞价 {{ st?.auction_start ?? '08:45' }}–{{ st?.open }}，连续交易 {{ st?.open }}–{{ st?.close }}</p>
      <p class="small muted center">可用资金 <span class="num">{{ money(cash) }}</span></p>
    </template>
  </div>
</template>

<style scoped>
.op { display: flex; flex-direction: column; gap: 10px; }
.quick { display: flex; gap: 6px; }
.quick .btn { flex: 1; }
.est { margin: 0; display: flex; flex-direction: column; gap: 2px; color: var(--ink-2); }
.est div { display: flex; justify-content: space-between; }
.est dt, .est dd { margin: 0; }
.est .tot { color: var(--ink); font-weight: 600; border-top: 1px solid var(--rule); padding-top: 4px; margin-top: 2px; }
.center { text-align: center; }
.seg button:disabled { opacity: 0.4; cursor: not-allowed; }
</style>
