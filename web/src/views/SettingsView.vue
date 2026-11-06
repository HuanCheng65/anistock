<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { api, call, errorText, type BindCode } from '../api'
import BackButton from '../components/BackButton.vue'
import Icon from '../components/Icon.vue'
import { hm } from '../format'
import { KYC_UNLOCKS } from '../kyc'
import { useAccount } from '../stores/account'
import { useCredit } from '../stores/credit'
import { usePrefs, type Theme } from '../stores/prefs'
import { useSession } from '../stores/session'
import { markEditing } from '../version'

const prefs = usePrefs()
const session = useSession()
const bot = computed(() => session.rules?.contact.qq_bot)
const account = useAccount()
const router = useRouter()
const themes: [Theme, string][] = [['system', '跟随系统'], ['light', '浅色'], ['dark', '深色']]
const docs = [['terms', '用户协议'], ['privacy', '隐私政策'], ['disclosure', '风险揭示书'], ['margin_disclosure', '融资融券风险揭示书']] as const
// ---- 修改密码: the new cookie comes back with the answer; other devices sign in again
const changing = ref(false)
const curPw = ref('')
const newPw = ref('')
const pwMsg = ref<{ kind: 'ok' | 'error'; text: string } | null>(null)
async function changePassword() {
  if (newPw.value.length < 6) return (pwMsg.value = { kind: 'error', text: '新密码至少 6 位' })
  busy.value = true
  pwMsg.value = null
  try {
    session.me = await call(api.POST('/api/v1/me/password', { body: { current_password: curPw.value, new_password: newPw.value } }))
    changing.value = false
    curPw.value = ''
    newPw.value = ''
    pwMsg.value = { kind: 'ok', text: '已修改密码，其他设备需要重新登录' }
  } catch (e) {
    pwMsg.value = { kind: 'error', text: errorText(e) }
  } finally {
    busy.value = false
  }
}

// ---- 注销账号 (docs/v2-account.md §9): the password, then the account is gone for good
const closing = ref(false)
const closePw = ref('')
const closeErr = ref('')
const closeForm = ref<HTMLElement | null>(null)
watch(closing, async (v) => {
  markEditing('close', v)
  if (v) {
    await nextTick()
    closeForm.value?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }
})
async function closeAccount() {
  busy.value = true
  closeErr.value = ''
  try {
    await call(api.DELETE('/api/v1/me', { body: { password: closePw.value, confirm: true } }))
    session.me = null
    account.clear()
    useCredit().clear()
    router.replace('/')
  } catch (e) {
    closeErr.value = errorText(e)
  } finally {
    busy.value = false
  }
}

async function logout() {
  await session.logout()
  account.clear()
  useCredit().clear()
  router.push('/')
}

// ---- 身份验证: a QQ binding today (backend/src/kyc.rs); a one-time code for the bot (/安易 绑定 <码>)
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
  if (code.value && now.value % 5000 < 1000) {
    account.refreshBindings().then(() => {
      if (!binding.value) return
      code.value = null
      session.loadMe().catch(() => {})
    }).catch(() => {})
  }
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
    session.loadMe().catch(() => {})
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
      <h2>身份验证</h2>
      <template v-if="binding">
        <div class="acct">
          <span>已通过 QQ 完成</span>
          <button v-if="confirmId !== binding.id" type="button" class="btn ghost small" @click="confirmId = binding.id">解除绑定</button>
        </div>
        <div v-if="confirmId === binding.id" class="confirm" role="group" aria-labelledby="unbind-title">
          <h3 id="unbind-title">解除 QQ 绑定？</h3>
          <p>身份验证随之取消，不能领取财政补贴、认购新股、进入排行或开通融资融券，也不能通过机器人找回密码。</p>
          <div class="acts">
            <button type="button" class="btn small" :disabled="busy" @click="unbind(binding.id)">解除绑定</button>
            <button type="button" class="btn small ghost" :disabled="busy" @click="confirmId = null">取消</button>
          </div>
        </div>
      </template>
      <template v-else>
        <p class="hint">买卖股票不需要身份验证。以下功能需要先完成：</p>
        <dl class="unlocks">
          <div v-for="[k, v] in KYC_UNLOCKS" :key="k"><dt>{{ k }}</dt><dd>{{ v }}</dd></div>
        </dl>
        <p class="hint">一个 QQ 只能验证一个账号。在 QQ 里私聊<a v-if="bot" :href="bot" target="_blank" rel="noopener">机器人</a><template v-else>机器人</template>，发送“/安易 绑定 绑定码”。</p>
        <div v-if="code" class="code">
          <span class="num big">{{ code.code }}</span>
          <span class="muted">{{ hm(code.expires_at) }} 前有效</span>
        </div>
        <button type="button" class="btn small" :disabled="busy" @click="makeCode">{{ code ? '重新生成' : '生成绑定码' }}</button>
      </template>
      <p v-if="msg" class="msg" :class="msg.kind === 'error' ? 'error' : 'ok'" role="status">{{ msg.text }}</p>
    </section>
    <section v-if="session.me" class="sec">
      <h2>交易</h2>
      <div class="acct">
        <span id="confirm-orders">下单前确认</span>
        <button type="button" role="switch" class="switch" :aria-checked="prefs.confirmOrders" aria-labelledby="confirm-orders"
          @click="prefs.confirmOrders = !prefs.confirmOrders"><span></span></button>
      </div>
    </section>
    <section class="sec">
      <h2>协议</h2>
      <nav class="docs">
        <RouterLink v-for="[k, l] in docs" :key="k" :to="`/legal/${k}`" class="docrow">
          <span>{{ l }}</span>
          <Icon name="chevron" :size="16" class="chev" />
        </RouterLink>
      </nav>
    </section>
    <section v-if="session.me" class="sec">
      <h2>账号</h2>
      <div class="acct">
        <span>{{ session.me.username }}</span>
        <button type="button" class="btn ghost small" @click="logout">退出登录</button>
      </div>
      <button v-if="!changing" type="button" class="linkbtn pw" @click="changing = true; pwMsg = null">修改密码</button>
      <form v-else class="confirm" @submit.prevent="changePassword">
        <input v-model="curPw" class="input" type="password" autocomplete="current-password" placeholder="当前密码" />
        <input v-model="newPw" class="input" type="password" autocomplete="new-password" placeholder="新密码，至少 6 位" />
        <div class="acts">
          <button type="submit" class="btn small" :disabled="busy || !curPw || !newPw">修改密码</button>
          <button type="button" class="btn small ghost" :disabled="busy" @click="changing = false; curPw = ''; newPw = ''; pwMsg = null">取消</button>
        </div>
      </form>
      <p v-if="pwMsg" class="msg" :class="pwMsg.kind === 'error' ? 'error' : 'ok'" role="status">{{ pwMsg.text }}</p>
      <button v-if="!closing" type="button" class="linkbtn close" @click="closing = true">注销账号</button>
      <form v-else ref="closeForm" class="confirm" role="group" aria-labelledby="close-title" @submit.prevent="closeAccount">
        <h3 id="close-title">注销账号？</h3>
        <p>持仓按最新价转出，资金清零，个人信息删除。注销后账号无法恢复。</p>
        <input v-model="closePw" class="input" type="password" autocomplete="current-password" placeholder="输入密码确认" />
        <p v-if="closeErr" class="msg error" role="alert">{{ closeErr }}</p>
        <div class="acts">
          <button type="submit" class="btn small" :disabled="busy || !closePw">注销账号</button>
          <button type="button" class="btn small ghost" :disabled="busy" @click="closing = false; closePw = ''; closeErr = ''">取消</button>
        </div>
      </form>
    </section>
    <p class="foot">星辉是安易内的虚拟资产，与真实货币无关，不能充值或提现。作品数据来自 AniList、Bangumi、Annict 的公开统计。</p>
  </main>
</template>

<style scoped>
.opts { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); border: 1px solid var(--base-line); }
.opts button { height: 40px; border: 0; background: transparent; font-size: 14px; color: var(--ink-2); }
.opts button + button { border-left: 1px solid var(--base-line); }
.opts button[aria-checked="true"] { background: var(--ink); color: var(--ground); font-weight: 700; }
.acct { display: flex; align-items: center; justify-content: space-between; font-size: 15px; }
.hint { font-size: 13px; color: var(--ink-2); line-height: 1.6; margin: 0 0 10px; }
.hint a { color: var(--ink); text-decoration: underline; text-underline-offset: 3px; }
.unlocks { margin: 0 0 12px; border-top: 1px solid var(--hair-2); }
.unlocks div { display: flex; align-items: baseline; justify-content: space-between; gap: 12px; padding: 8px 0; border-bottom: 1px solid var(--hair-2); font-size: 14px; }
.unlocks dd { margin: 0; font-size: 12px; color: var(--muted); text-align: right; }
.code { display: flex; align-items: baseline; gap: 12px; margin: 4px 0 12px; }
.code .big { font-size: 28px; font-weight: 600; letter-spacing: 0.08em; }
.code .muted { font-size: 12px; }
.confirm { display: flex; flex-direction: column; gap: 6px; padding: 12px 0 0; }
.confirm h3 { margin: 0; font-size: 15px; font-weight: 700; }
.confirm p { margin: 0; font-size: 13px; color: var(--ink-2); }
.acts { display: flex; gap: 8px; margin-top: 6px; }
.switch { position: relative; width: 44px; height: 26px; flex: none; padding: 0; border: 1px solid var(--base-line); background: var(--field-bg); transition: background-color 0.2s ease; }
.switch span { position: absolute; top: 3px; left: 3px; width: 18px; height: 18px; background: var(--muted); transition: transform 0.2s cubic-bezier(0.2, 0.7, 0.2, 1), background-color 0.2s ease; }
.switch[aria-checked="true"] { background: var(--ink); border-color: var(--ink); }
.switch[aria-checked="true"] span { transform: translateX(18px); background: var(--ground); }
@media (prefers-reduced-motion: reduce) { .switch, .switch span { transition: none; } }
.docs { margin-top: -6px; }
.pw { display: block; margin-top: 14px; color: var(--ink-2); }
.close { display: block; margin-top: 14px; color: var(--muted); }
.confirm .input { height: 40px; font-size: 15px; margin-top: 4px; }
.docrow { display: flex; align-items: center; justify-content: space-between; padding: 12px 0; border-bottom: 1px solid var(--hair-2); font-size: 15px; }
.docrow:last-child { border-bottom: 0; }
.chev { color: var(--muted); }
</style>
