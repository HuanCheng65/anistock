<script setup lang="ts">
// One IPO: terms, demand, and the player's subscription (freeze qty × offer price; edit or withdraw until close).
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute } from 'vue-router'
import { api, ApiError, type Ipo } from '../api'
import { money } from '../format'
import { useSession } from '../stores/session'

const props = defineProps<{ ipo: Ipo; compact?: boolean }>()
const emit = defineEmits<{ changed: [] }>()
const session = useSession()
const route = useRoute()
const qty = ref(props.ipo.my?.qty ? String(props.ipo.my.qty) : '')
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
watch(() => props.ipo.my?.qty, (q) => { if (!busy.value) qty.value = q ? String(q) : '' })

const heat = computed(() => (props.ipo.base_public_qty ? props.ipo.applied_qty / props.ipo.base_public_qty : 0))
const n = computed(() => {
  const v = Number(qty.value)
  return Number.isInteger(v) && v >= 0 ? v : -1
})
const freeze = computed(() => (n.value > 0 ? n.value * props.ipo.ipo_price : 0))
const mine = computed(() => props.ipo.my?.qty ?? 0)
// cash available for this subscription = free cash + what this subscription already froze
const room = computed(() => (session.me?.available ?? 0) + mine.value * props.ipo.ipo_price)
const maxQty = computed(() => Math.floor(room.value / props.ipo.ipo_price))
const problem = computed(() => {
  if (qty.value === '') return ''
  if (n.value < 0) return '数量必须是整数'
  if (n.value > props.ipo.float_qty) return `最多认购 ${props.ipo.float_qty} 股（全部流通盘）`
  if (freeze.value > room.value + 1e-9) return `资金不够，最多认购 ${maxQty.value} 股`
  return ''
})
const open = computed(() => props.ipo.status === 'open')
const tier = computed(() => {
  const h = heat.value
  if (h >= 100) return '超额 100 倍以上，公开发售回拨到流通盘的 90%'
  if (h >= 50) return '超额 50 倍以上，回拨到 80%'
  if (h >= 15) return '超额 15 倍以上，回拨到 70%'
  return '公开发售为流通盘的 50%'
})

async function submit(q: number) {
  if (busy.value) return
  busy.value = true
  msg.value = null
  try {
    if (q === 0) await api.del(`/api/ipos/${props.ipo.id}/subscribe`)
    else await api.post(`/api/ipos/${props.ipo.id}/subscribe`, { qty: q })
    msg.value = { kind: 'ok', text: q === 0 ? '已撤回认购，冻结资金已退回' : `已认购 ${q} 股，冻结 ${money(q * props.ipo.ipo_price)} 币` }
    if (q === 0) qty.value = ''
    await session.loadMe()
    emit('changed')
  } catch (e) {
    msg.value = { kind: 'error', text: e instanceof ApiError ? (e.status === 404 ? '认购功能还没开放' : e.message) : '提交失败' }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <article class="ipo">
    <header v-if="!compact">
      <RouterLink :to="`/stock/${ipo.id}`" class="t">{{ ipo.title }}</RouterLink>
      <span v-if="ipo.title_ja && ipo.title_ja !== ipo.title" class="small muted ja">{{ ipo.title_ja }}</span>
      <span class="small muted num">{{ ipo.code }}</span>
    </header>
    <dl class="terms small">
      <div><dt>发行价</dt><dd class="num strong">{{ money(ipo.ipo_price) }}</dd></div>
      <div><dt>认购截止</dt><dd class="num">{{ ipo.sub_end }} 日结</dd></div>
      <div><dt>上市</dt><dd class="num">{{ ipo.listed_on }}</dd></div>
      <div><dt>首播</dt><dd class="num">{{ ipo.first_air }}</dd></div>
      <div><dt>流通盘</dt><dd class="num">{{ ipo.float_qty.toLocaleString('zh-CN') }} 股</dd></div>
      <div><dt>认购热度</dt><dd class="num" :class="heat >= 1 ? 'up' : ''">{{ heat.toFixed(1) }} 倍 · {{ ipo.applicants }} 人</dd></div>
    </dl>

    <div class="heatbar" role="img" :aria-label="`认购总量 ${ipo.applied_qty} 股，公开发售基础 ${ipo.base_public_qty} 股`">
      <span :style="{ width: Math.min(100, heat * 100) + '%' }"></span>
    </div>
    <p class="small muted">{{ open ? tier + '。超额时按比例分配，零头抽签。' : '' }}</p>

    <template v-if="open">
      <template v-if="session.me">
        <label class="field">
          <span class="label">认购股数 · 最多 <span class="num">{{ Math.min(maxQty, ipo.float_qty) }}</span></span>
          <input :id="`ipo-qty-${ipo.id}`" class="input num" inputmode="numeric" v-model="qty" placeholder="输入股数" autocomplete="off" />
        </label>
        <p class="small muted">需冻结 <b class="num">{{ money(freeze) }}</b> 币 · 可用 <span class="num">{{ money(room) }}</span>。没分到的部分在分配后退回。</p>
        <p v-if="problem" class="notice error">{{ problem }}</p>
        <div class="acts">
          <button type="button" class="btn primary" :disabled="busy || !!problem || n <= 0 || n === mine" @click="submit(n)">
            {{ mine ? '修改认购' : '认购' }}
          </button>
          <button v-if="mine" type="button" class="btn" :disabled="busy" @click="submit(0)">撤回</button>
        </div>
        <p v-if="mine" class="small">已认购 <b class="num">{{ mine }}</b> 股</p>
      </template>
      <RouterLink v-else :to="{ name: 'login', query: { next: route.fullPath } }" class="btn primary block">登录后认购</RouterLink>
    </template>

    <div v-else-if="ipo.result" class="result small">
      <span>超额认购 <b class="num">{{ ipo.result.oversubscribed.toFixed(1) }}</b> 倍，公开发售 <b class="num">{{ ipo.result.public_qty }}</b> 股，分配比例 <b class="num">{{ (ipo.result.ratio * 100).toFixed(1) }}%</b></span>
      <span v-if="ipo.my">我认购 {{ ipo.my.qty }} 股，<b :class="ipo.my.allotted ? 'up' : ''">获配 {{ ipo.my.allotted ?? 0 }} 股</b></span>
    </div>
    <p v-if="msg" class="notice" :class="msg.kind" role="status">{{ msg.text }}</p>
  </article>
</template>

<style scoped>
.ipo { display: flex; flex-direction: column; gap: 10px; }
header { display: flex; flex-direction: column; gap: 1px; }
.t { font-weight: 600; font-size: 16px; color: var(--ink); }
.ja { overflow-wrap: anywhere; }
.terms { margin: 0; display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px 10px; }
.terms div { display: flex; flex-direction: column; }
.terms dt { color: var(--muted); }
.terms dd { margin: 0; }
.strong { font-weight: 700; font-size: 15px; }
.heatbar { height: 6px; background: var(--surface-2); border-radius: 3px; overflow: hidden; }
.heatbar span { display: block; height: 100%; background: var(--seal); }
.acts { display: flex; gap: 8px; }
.acts .btn { flex: 1; }
.result { display: flex; flex-direction: column; gap: 4px; padding: 8px; background: var(--surface-2); border-radius: 4px; }
</style>
