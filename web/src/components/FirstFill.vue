<script setup lang="ts">
// 首笔成交: after a player's first fill, once, what happens next to the shares (the price moves, the dividends,
// their tax). Seen is kept on the server (me.onboarding.marks `first_fill`).
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { api, call } from '../api'
import { money, qty } from '../format'
import { useAccount } from '../stores/account'
import { useGuide } from '../stores/guide'
import { useSession } from '../stores/session'
import Modal from './Modal.vue'

const session = useSession()
const account = useAccount()
const guide = useGuide()
const router = useRouter()
const busy = ref(false)
const open = computed(() => {
  const o = session.me?.onboarding
  return !!o && o.traded && !o.marks.includes('first_fill') && !guide.mark
})
const fill = computed(() => account.lastFill)

async function done(to?: string) {
  const me = session.me
  if (!me || busy.value) return
  busy.value = true
  me.onboarding = { ...me.onboarding, marks: [...me.onboarding.marks, 'first_fill'] }
  try {
    session.me = await call(api.POST('/api/v1/me/marks/{key}', { params: { path: { key: 'first_fill' } } }))
  } catch {
    /* hidden for now */
  } finally {
    busy.value = false
  }
  if (to) router.push(to)
}
</script>

<template>
  <Modal :open="open" title="首笔成交" :busy="busy" @close="done()">
    <p v-if="fill" class="num fill">{{ fill.side === 'buy' ? '买入' : '卖出' }}成交 {{ qty(fill.quantity) }} 股 · {{ money(fill.price) }}</p>
    <ul class="pts">
      <li>股价随买卖委托变动，持仓盈亏随之变化。</li>
      <li>分红每周日按持仓股数发放，计入可用资金。</li>
      <li>持有满 4 周的分红免税，不满 4 周交 10%，当天买入的交 20%。</li>
    </ul>
    <template #actions>
      <button type="button" class="btn" :disabled="busy" @click="done('/me')">查看持仓</button>
      <button type="button" class="btn ghost" :disabled="busy" @click="done()">关闭</button>
    </template>
  </Modal>
</template>

<style scoped>
.fill { font-size: 13px; color: var(--muted); margin-bottom: 6px; }
.pts { border-top: 1px solid var(--hair-2); }
.pts li { padding: 9px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; line-height: 1.65; color: var(--ink-2); }
</style>
