<script setup lang="ts">
// 破产重整: a quiet entry on 我的 while the player qualifies, with an in-page confirmation.
import { computed, ref, watch } from 'vue'
import { api, call, errorText, type Me } from '../api'
import { money } from '../format'
import { useSession } from '../stores/session'
import { markEditing } from '../version'

const props = defineProps<{ me: Me }>()
const session = useSession()
const open = ref(false)
const busy = ref(false)
const error = ref('')

// qualifies by assets and season; outside trading hours the entry stays, the button waits
const r = computed(() => props.me.restructure)
const shown = computed(() => r.value.eligible || r.value.reason === 'market_closed')
const initial = computed(() => money(props.me.initial_cash, 0))
watch(open, (o) => markEditing('restructure', o))

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
    <button v-if="!open" type="button" class="entry" @click="open = true">
      <span>破产重整</span>
      <span class="hint">本季可申请 1 次</span>
    </button>
    <div v-else class="confirm" role="group" aria-labelledby="rs-title">
      <h2 id="rs-title">申请破产重整？</h2>
      <p>全部持仓按现价卖出。</p>
      <p>委托和新股认购全部撤销。</p>
      <p>资金补足到 {{ initial }} 星辉。</p>
      <p>排行的收益率继续计入此前的亏损。</p>
      <p>每季只能申请一次。</p>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <p v-else-if="!r.eligible && r.message" class="wait">{{ r.message }}</p>
      <div class="acts">
        <button type="button" class="btn small" :disabled="busy || !r.eligible" @click="confirm">
          {{ busy ? '处理中' : '申请重整' }}
        </button>
        <button type="button" class="btn small ghost" :disabled="busy" @click="open = false; error = ''">取消</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.rs { border-bottom: 1px solid var(--hair-2); }
.entry { display: flex; align-items: baseline; width: 100%; padding: 12px 0; border: 0; background: none; color: var(--ink-2); font-size: 14px; text-align: left; }
.hint { margin-left: auto; font-size: 12px; color: var(--muted); }
.confirm { padding: 14px 0; display: flex; flex-direction: column; gap: 6px; }
.confirm h2 { margin: 0 0 4px; font-size: 15px; font-weight: 700; color: var(--ink); }
.confirm p { margin: 0; font-size: 13px; line-height: 1.6; }
.confirm p:not(.msg) { color: var(--ink-2); }
.confirm .wait { color: var(--muted); }
.acts { display: flex; gap: 8px; margin-top: 8px; }
</style>
