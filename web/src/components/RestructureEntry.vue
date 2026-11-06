<script setup lang="ts">
// 破产重整: a quiet entry on 我的 while the player qualifies; the confirmation opens in a dialog.
import { computed, ref } from 'vue'
import { api, call, errorText, type Me } from '../api'
import { money } from '../format'
import Modal from './Modal.vue'
import { useSession } from '../stores/session'

const props = defineProps<{ me: Me }>()
const session = useSession()
const open = ref(false)
const busy = ref(false)
const error = ref('')

// qualifies by assets and season; outside trading hours the entry stays, the button waits
const r = computed(() => props.me.restructure)
const shown = computed(() => r.value.eligible || r.value.reason === 'market_closed')
const initial = computed(() => money(props.me.initial_cash, 0))

async function confirm() {
  busy.value = true
  error.value = ''
  try {
    session.me = (await call(api.POST('/api/v1/me/restructure', { body: { confirm: true } }))).me
    open.value = false
    session.loadLeaders().catch(() => {})
  } catch (e) {
    error.value = errorText(e, '无法申请，稍后重试')
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <section v-if="shown" class="rs gutter">
    <button type="button" class="entry" @click="open = true">
      <span>破产重整</span>
      <span class="hint">本季可申请 1 次</span>
    </button>
    <Modal :open="open" title="申请破产重整？" edit-key="restructure" :busy="busy" @close="open = false; error = ''">
      <p>全部持仓按现价卖出。</p>
      <p>委托和新股认购全部撤销。</p>
      <p>资金补足到 {{ initial }} 星辉。</p>
      <p>排行的收益率继续计入此前的亏损。</p>
      <p>每季只能申请一次。</p>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <p v-else-if="!r.eligible && r.message" class="wait">{{ r.message }}</p>
      <template #actions>
        <button type="button" class="btn" :disabled="busy || !r.eligible" @click="confirm">{{ busy ? '处理中' : '申请重整' }}</button>
        <button type="button" class="btn ghost" :disabled="busy" @click="open = false; error = ''">取消</button>
      </template>
    </Modal>
  </section>
</template>

<style scoped>
.rs { border-bottom: 1px solid var(--hair-2); }
.entry { display: flex; align-items: baseline; width: 100%; padding: 12px 0; border: 0; background: none; color: var(--ink-2); font-size: 14px; text-align: left; }
.hint { margin-left: auto; font-size: 12px; color: var(--muted); }
.wait { color: var(--muted); }
</style>
