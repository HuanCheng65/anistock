<script setup lang="ts">
// A rights issue in its subscription days (docs/design/corporate.md §3.3): terms and, for a holder, the 配股认购 form.
// Rights cannot be traded; what is not subscribed is taken by the rights holders at the ex-rights price and the
// difference is paid to the holder, so not acting is not a loss.
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, call, errorText, type RightsOffer } from '../api'
import { money, qty as fmtQty, shortDay } from '../format'
import { useSession } from '../stores/session'

const props = defineProps<{ stockId: number; offer: RightsOffer }>()
const emit = defineEmits<{ changed: [] }>()
const session = useSession()
const route = useRoute()
const a = computed(() => props.offer.action!)
const qty = ref(props.offer.subscribed ? String(props.offer.subscribed) : '')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
watch(() => props.offer.subscribed, (q) => { if (!busy.value) qty.value = q ? String(q) : '' })

const perTen = computed(() => {
  const x = Math.round((a.value.rights_ratio ?? 0) * 100) / 10
  return Number.isInteger(x) ? String(x) : x.toFixed(1)
})
const price = computed(() => a.value.rights_price ?? 0)
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v >= 0 ? v : -1
})
const mine = computed(() => props.offer.subscribed)
const room = computed(() => (session.me?.available_cash ?? 0) + props.offer.frozen)
const problem = computed(() => {
  if (qty.value === '') return ''
  if (n.value < 0) return '数量需为整数'
  if (n.value > props.offer.entitlement) return `最多认购 ${fmtQty(props.offer.entitlement)} 股`
  if (n.value * price.value > room.value) return '可用资金不足'
  return ''
})
const closeText = computed(() => {
  const t = session.status?.trading_day
  return a.value.effective === t ? '今天收盘后' : `${shortDay(a.value.effective, t)} 收盘后`
})

async function submit(q: number) {
  if (busy.value) return
  busy.value = true
  msg.value = null
  try {
    if (q === 0) await call(api.DELETE('/api/v1/stocks/{id}/rights/subscription', { params: { path: { id: props.stockId } } }))
    else await call(api.PUT('/api/v1/stocks/{id}/rights/subscription', { params: { path: { id: props.stockId } }, body: { quantity: q } }))
    msg.value = { kind: 'ok', text: q === 0 ? '已撤回认购，资金已解冻' : `已认购 ${fmtQty(q)} 股` }
    if (q === 0) qty.value = ''
    await session.loadMe()
    emit('changed')
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e, '无法提交认购') }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <article class="rights">
    <dl class="facts">
      <div><dt>配股比例</dt><dd>每 10 股配 {{ perTen }} 股</dd></div>
      <div><dt>配股价</dt><dd>{{ money(price) }}</dd></div>
      <div><dt>登记缴款</dt><dd>{{ closeText }}</dd></div>
    </dl>
    <form class="term form" @submit.prevent="submit(n)">
      <template v-if="session.me">
        <template v-if="offer.entitlement > 0 || mine > 0">
          <label class="row">认购数量
            <input class="tinput num" inputmode="numeric" v-model="qty" autocomplete="off" :placeholder="`最多 ${fmtQty(offer.entitlement)}`" />
          </label>
          <div class="kv"><span>冻结资金</span><span class="num v">{{ money(n > 0 ? n * price : 0) }}</span></div>
          <p v-if="problem" class="note err">{{ problem }}</p>
          <div class="acts">
            <button type="submit" class="tbtn buy" :disabled="busy || !!problem || n <= 0 || n === mine">{{ mine ? '修改认购' : '认购' }}</button>
            <button v-if="mine" type="button" class="tbtn plain" :disabled="busy" @click="submit(0)">撤回</button>
          </div>
          <p v-if="mine && !msg" class="note ok">已认购 <span class="num">{{ fmtQty(mine) }}</span> 股</p>
          <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">{{ msg.text }}</p>
        </template>
        <p v-else class="note">持有这支股票才有配股权</p>
      </template>
      <RouterLink v-else :to="{ name: 'login', query: { next: route.fullPath } }" class="tbtn buy cta">登录后认购</RouterLink>
      <p class="note">登记时按持仓重新计算可认购数量，超出部分退回。不认购的部分由制作委员会按除权参考价接手，差价以现金补给你。</p>
    </form>
  </article>
</template>

<style scoped>
.rights { display: flex; flex-direction: column; gap: 14px; }
.facts { border-top: 1px solid var(--hair); border-bottom: 1px solid var(--hair); }
.form { display: flex; flex-direction: column; gap: 10px; padding: 14px; }
.row { display: grid; grid-template-columns: 64px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; color: var(--t-ink-2); }
.row .tinput { height: 38px; font-size: 16px; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.acts { display: flex; gap: 8px; }
.acts .tbtn { flex: 1; height: 44px; }
.cta { height: 44px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.err { color: var(--t-danger); }
.note.ok { color: var(--t-ink); }
</style>
