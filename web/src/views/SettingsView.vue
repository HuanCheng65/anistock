<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { api, call, errorText, type BindCode } from '../api'
import BackButton from '../components/BackButton.vue'
import { hm } from '../format'
import { useAccount } from '../stores/account'
import { usePrefs, type Theme } from '../stores/prefs'
import { useSession } from '../stores/session'
import { markEditing } from '../version'

const prefs = usePrefs()
const session = useSession()
const account = useAccount()
const router = useRouter()
const themes: [Theme, string][] = [['system', '跟随系统'], ['light', '浅色'], ['dark', '深色']]
async function logout() {
  await session.logout()
  account.clear()
  router.push('/')
}

// ---- QQ 绑定: a one-time code for the club bot (/绑定 <码> in the group)
const code = ref<BindCode | null>(null)
const busy = ref(false)
const msg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
const confirmId = ref<number | null>(null)
watch(confirmId, (v) => markEditing('unbind', v != null))
onBeforeUnmount(() => markEditing('unbind', false))
const binding = computed(() => (account.bindings ?? []).find((b) => b.platform === 'qq') ?? null)
if (session.me) account.refreshBindings().catch(() => {})
// the code expires after 10 minutes; drop it then
const now = ref(Date.now())
const timer = window.setInterval(() => {
  now.value = Date.now()
  if (code.value && Date.parse(code.value.expires_at) <= now.value) code.value = null
  // the bot redeems the code: show the binding as soon as it exists
  if (code.value && now.value % 5000 < 1000) account.refreshBindings().then(() => binding.value && (code.value = null)).catch(() => {})
}, 1000)
onBeforeUnmount(() => window.clearInterval(timer))

async function makeCode() {
  busy.value = true
  msg.value = null
  try {
    code.value = await call(api.POST('/api/v1/me/bind-codes', { body: {} }))
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e) }
  } finally {
    busy.value = false
  }
}
async function unbind(id: number) {
  busy.value = true
  msg.value = null
  try {
    await call(api.DELETE('/api/v1/me/bindings/{id}', { params: { path: { id } } }))
    confirmId.value = null
    await account.refreshBindings()
    msg.value = { kind: 'ok', text: '已解除绑定' }
  } catch (e) {
    msg.value = { kind: 'error', text: errorText(e) }
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="page doc">
    <header class="pagehead back">
      <BackButton />
      <h1>设置</h1>
    </header>
    <section class="sec">
      <h2>外观</h2>
      <div class="opts" role="radiogroup" aria-label="外观">
        <button v-for="[k, l] in themes" :key="k" type="button" role="radio" :aria-checked="prefs.theme === k" @click="prefs.theme = k">{{ l }}</button>
      </div>
    </section>
    <section v-if="session.me" class="sec">
      <h2>QQ 绑定</h2>
      <template v-if="binding">
        <div class="acct">
          <span>QQ <span class="num">{{ binding.display_name }}</span></span>
          <button v-if="confirmId !== binding.id" type="button" class="btn ghost small" @click="confirmId = binding.id">解除绑定</button>
        </div>
        <div v-if="confirmId === binding.id" class="confirm" role="group" aria-labelledby="unbind-title">
          <h3 id="unbind-title">解除 QQ 绑定？</h3>
          <p>群里的机器人将无法查看这个账号。</p>
          <div class="acts">
            <button type="button" class="btn small" :disabled="busy" @click="unbind(binding.id)">解除绑定</button>
            <button type="button" class="btn small ghost" :disabled="busy" @click="confirmId = null">取消</button>
          </div>
        </div>
      </template>
      <template v-else>
        <p class="hint">在社团群里发送“/绑定 绑定码”，机器人即可查看你的资产和持仓。</p>
        <div v-if="code" class="code">
          <span class="num big">{{ code.code }}</span>
          <span class="muted">{{ hm(code.expires_at) }} 前有效</span>
        </div>
        <button type="button" class="btn small" :disabled="busy" @click="makeCode">{{ code ? '重新生成' : '生成绑定码' }}</button>
      </template>
      <p v-if="msg" class="msg" :class="msg.kind === 'error' ? 'error' : 'ok'" role="status">{{ msg.text }}</p>
    </section>
    <section v-if="session.me" class="sec">
      <h2>账号</h2>
      <div class="acct">
        <span>{{ session.me.username }}</span>
        <button type="button" class="btn ghost small" @click="logout">退出登录</button>
      </div>
    </section>
    <p class="foot">游戏币与真实货币无关，不能充值或提现。作品数据来自 AniList、Bangumi、Annict 的公开统计。</p>
  </main>
</template>

<style scoped>
.opts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); border: 1px solid var(--base-line); }
.opts button { height: 40px; border: 0; background: transparent; font-size: 14px; color: var(--ink-2); }
.opts button + button { border-left: 1px solid var(--base-line); }
.opts button[aria-checked="true"] { background: var(--ink); color: var(--ground); font-weight: 700; }
.acct { display: flex; align-items: center; justify-content: space-between; font-size: 15px; }
.hint { font-size: 13px; color: var(--ink-2); line-height: 1.6; margin: 0 0 10px; }
.code { display: flex; align-items: baseline; gap: 12px; margin: 4px 0 12px; }
.code .big { font-size: 28px; font-weight: 600; letter-spacing: 0.08em; }
.code .muted { font-size: 12px; }
.confirm { display: flex; flex-direction: column; gap: 6px; padding: 12px 0 0; }
.confirm h3 { margin: 0; font-size: 15px; font-weight: 700; }
.confirm p { margin: 0; font-size: 13px; color: var(--ink-2); }
.acts { display: flex; gap: 8px; margin-top: 6px; }
</style>
