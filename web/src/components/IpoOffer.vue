<script setup lang="ts">
// One IPO: terms, demand against the clawback tiers, and the subscription form (terminal surface).
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { ApiError, type Ipo } from '../api'
import { money, qty as fmtQty, shortDay, share } from '../format'
import { useIpo } from '../stores/ipo'
import { useSession } from '../stores/session'
import Cover from './Cover.vue'

const props = defineProps<{ ipo: Ipo; episodes?: number | null; compact?: boolean }>()
const emit = defineEmits<{ changed: [] }>()
const session = useSession()
const store = useIpo()
const route = useRoute()
const qty = ref(props.ipo.my?.qty ? String(props.ipo.my.qty) : '')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
watch(() => props.ipo.my?.qty, (q) => { if (!busy.value) qty.value = q ? String(q) : '' })

const heat = computed(() => (props.ipo.base_public_qty ? props.ipo.applied_qty / props.ipo.base_public_qty : 0))
const publicShare = computed(() => (heat.value >= 100 ? 0.9 : heat.value >= 50 ? 0.8 : heat.value >= 15 ? 0.7 : 0.5))
const publicQty = computed(() => Math.round(props.ipo.float_qty * publicShare.value))
const heatX = computed(() => {
  const h = heat.value
  const seg = (a: number, b: number, x0: number, x1: number) => x0 + ((h - a) / (b - a)) * (x1 - x0)
  if (h <= 15) return seg(0, 15, 0, 34)
  if (h <= 50) return seg(15, 50, 34, 62)
  if (h <= 100) return seg(50, 100, 62, 90)
  return Math.min(100, seg(100, 300, 90, 100))
})

const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v >= 0 ? v : -1
})
const mine = computed(() => props.ipo.my?.qty ?? 0)
const freeze = computed(() => (n.value > 0 ? n.value * props.ipo.ipo_price : 0))
const room = computed(() => (session.me?.available ?? 0) + mine.value * props.ipo.ipo_price)
const maxQty = computed(() => Math.min(props.ipo.float_qty, Math.floor(room.value / props.ipo.ipo_price)))
const expected = computed(() => {
  if (n.value <= 0) return 0
  const applied = props.ipo.applied_qty - mine.value + n.value
  return applied <= publicQty.value ? n.value : Math.floor((n.value * publicQty.value) / applied)
})
const problem = computed(() => {
  if (qty.value === '') return ''
  if (n.value < 0) return '数量需为整数'
  if (n.value > props.ipo.float_qty) return `最多认购 ${fmtQty(props.ipo.float_qty)} 股`
  if (freeze.value > room.value + 1e-9) return `可用资金最多认购 ${fmtQty(maxQty.value)} 股`
  return ''
})
const open = computed(() => props.ipo.status === 'open')
const today = computed(() => session.status?.day)
const closeText = computed(() => {
  const d = props.ipo.sub_end
  const t = today.value
  if (t && d === t) return '今天 06:30'
  return `${shortDay(d, t)} 日结`
})

async function submit(q: number) {
  if (busy.value) return
  busy.value = true
  msg.value = null
  try {
    await store.subscribe(props.ipo.id, q)
    msg.value = { kind: 'ok', text: q === 0 ? '已撤回认购，资金已解冻' : `已认购 ${fmtQty(q)} 股` }
    if (q === 0) qty.value = ''
    await session.loadMe()
    emit('changed')
  } catch (e) {
    msg.value = { kind: 'error', text: e instanceof ApiError ? e.message : '无法提交认购' }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <article class="offer" :class="{ compact, band: !compact }">
    <Cover v-if="!compact" class="oc" :id="ipo.id" :title="ipo.title" :w="84" :h="118" :size="17" />
    <div class="info">
      <div v-if="!compact" class="ti">
        <span class="status desk-only">{{ open ? '认购中' : '已配售' }}</span>
        <RouterLink :to="`/stocks/${ipo.id}`" class="serif name">{{ ipo.title }}</RouterLink>
        <div class="meta phone-only">首播 <span class="num">{{ ipo.first_air.slice(5) }}</span><template v-if="episodes"> · 全 {{ episodes }} 集</template></div>
        <div class="price phone-only"><span class="num">{{ money(ipo.ipo_price) }}</span><span class="muted">发行价</span></div>
      </div>

      <dl class="facts" :class="{ five: !compact }">
        <div v-if="!compact" class="desk-only"><dt>发行价</dt><dd>{{ money(ipo.ipo_price) }}</dd></div>
        <div><dt>公开发售</dt><dd>{{ fmtQty(open ? publicQty : ipo.result?.public_qty ?? publicQty) }} 股</dd></div>
        <div><dt>截止</dt><dd>{{ closeText }}</dd></div>
        <div><dt>上市</dt><dd>{{ ipo.listed_on.slice(5) }}</dd></div>
        <div v-if="!compact" class="desk-only"><dt>首播</dt><dd>{{ ipo.first_air.slice(5) }}</dd></div>
      </dl>

      <div class="demand">
        <div class="dline">
          <span>超额认购 <b class="num">{{ heat.toFixed(1) }}</b> 倍</span>
          <span class="ink2">公开发售 <span class="num ink">{{ share(publicShare) }}</span></span>
        </div>
        <div class="heat" role="img" :aria-label="`超额认购 ${heat.toFixed(1)} 倍`">
          <div class="track"></div>
          <div class="fill" :style="{ width: heatX + '%' }"></div>
          <span v-for="t in [['15×', 34], ['50×', 62], ['100×', 90]]" :key="t[0]" class="tick num" :style="{ left: t[1] + '%' }">{{ t[0] }}</span>
        </div>
      </div>
    </div>

    <form v-if="open" class="term form" @submit.prevent="submit(n)">
      <template v-if="session.me">
        <label class="row">认购数量
          <input class="tinput num" inputmode="numeric" v-model="qty" autocomplete="off" :placeholder="`最多 ${fmtQty(maxQty)}`" />
        </label>
        <div class="kv"><span>冻结资金</span><span class="num v">{{ money(freeze) }}</span></div>
        <div class="kv"><span>预计中签</span><span class="num v">{{ n > 0 ? `约 ${fmtQty(expected)} 股` : '–' }}</span></div>
        <p v-if="problem" class="note err">{{ problem }}</p>
        <div class="acts">
          <button type="submit" class="tbtn buy" :disabled="busy || !!problem || n <= 0 || n === mine">{{ mine ? '修改认购' : '认购' }}</button>
          <button v-if="mine" type="button" class="tbtn plain" :disabled="busy" @click="submit(0)">撤回</button>
        </div>
        <p v-if="mine" class="note ok">已认购 <span class="num">{{ fmtQty(mine) }}</span> 股</p>
        <p v-if="msg" class="note" :class="msg.kind === 'error' ? 'err' : 'ok'" role="status">{{ msg.text }}</p>
      </template>
      <RouterLink v-else :to="{ name: 'login', query: { next: route.fullPath } }" class="tbtn buy cta">登录后认购</RouterLink>
      <p class="note">截止后按认购数量比例配售，余数抽签。未中签部分的资金在配售时解冻。</p>
    </form>

    <dl v-else-if="ipo.result" class="facts result">
      <div><dt>超额认购</dt><dd>{{ ipo.result.oversubscribed.toFixed(1) }} 倍</dd></div>
      <div><dt>中签率</dt><dd>{{ ipo.result.ratio != null ? share(ipo.result.ratio, 1) : '–' }}</dd></div>
      <div v-if="ipo.my"><dt>获配</dt><dd :class="ipo.my.allotted ? 'up' : ''">{{ fmtQty(ipo.my.allotted ?? 0) }} / {{ fmtQty(ipo.my.qty) }} 股</dd></div>
    </dl>
  </article>
</template>

<style scoped>
.offer { display: flex; flex-direction: column; gap: 16px; }
.offer.band { display: grid; grid-template-columns: 84px minmax(0, 1fr); column-gap: 14px; row-gap: 16px; }
.band .info { display: contents; }
.band .oc { grid-column: 1; }
.band .ti { grid-column: 2; }
.band .facts, .band .demand, .band .form, .band .result { grid-column: 1 / -1; }
.info { display: flex; flex-direction: column; gap: 16px; min-width: 0; }
.status { font-size: 12px; color: var(--brand); font-weight: 700; }
.ti { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 6px; }
.name { font-size: 21px; font-weight: 900; line-height: 1.3; color: var(--ink); overflow-wrap: anywhere; }
.name::before { content: '《'; }
.name::after { content: '》'; }
.meta { font-size: 13px; color: var(--ink-2); }
.price { margin-top: auto; display: flex; align-items: baseline; gap: 6px; }
.price .num { font-size: 26px; font-weight: 600; }
.price .muted { font-size: 12px; }
.facts { border-top: 1px solid var(--hair); border-bottom: 1px solid var(--hair); }
.demand { display: flex; flex-direction: column; gap: 8px; }
.dline { display: flex; justify-content: space-between; font-size: 13px; }
.dline b { font-weight: 600; }
.ink { color: var(--ink); }
.heat { position: relative; height: 26px; }
.track { position: absolute; left: 0; right: 0; top: 6px; height: 6px; background: var(--grid); }
.fill { position: absolute; left: 0; top: 6px; height: 6px; background: var(--brand); }
.tick { position: absolute; top: 0; height: 18px; border-left: 1px solid var(--ink); padding-left: 4px; font-size: 11px; color: var(--ink-2); line-height: 30px; }
.form { display: flex; flex-direction: column; gap: 10px; padding: 14px; }
.row { display: grid; grid-template-columns: 64px minmax(0, 1fr); align-items: center; gap: 8px; font-size: 12px; color: var(--t-ink-2); }
.row .tinput { height: 38px; font-size: 16px; }
.kv { display: flex; justify-content: space-between; font-size: 12px; color: var(--t-ink-2); }
.kv .v { color: var(--t-ink); }
.acts { display: flex; gap: 8px; }
.acts .tbtn { flex: 1; height: 44px; }
.cta { height: 44px; display: flex; align-items: center; justify-content: center; }
.note { font-size: 12px; line-height: 1.55; color: var(--t-ink-2); }
.note.err { color: var(--t-up); }
.note.ok { color: var(--t-ink); }
.result { border-bottom: 0; }
@media (min-width: 1024px) {
  .offer.band { grid-template-columns: 120px minmax(0, 1fr) 400px; column-gap: 32px; align-items: start; }
  .band .oc { width: 120px !important; height: 168px !important; }
  .band .info { display: flex; grid-column: 2; }
  .band .ti { gap: 4px; }
  .band .name { font-size: 28px; }
  .band .form, .band .result { grid-column: 3; grid-row: 1; }
  .facts.five { grid-template-columns: repeat(5, minmax(0, 1fr)); }
}
</style>
