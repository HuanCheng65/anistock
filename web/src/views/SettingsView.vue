<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { api, ApiError, call, errorText, type ApiKey, type ApiKeyCreated, type BindCode } from '../api'
import BackButton from '../components/BackButton.vue'
import Icon from '../components/Icon.vue'
import Modal from '../components/Modal.vue'
import { hm, when } from '../format'
import { KYC_UNLOCKS } from '../kyc'
import { useAccount } from '../stores/account'
import { useCredit } from '../stores/credit'
import { usePrefs, type Theme } from '../stores/prefs'
import { useSession } from '../stores/session'

const prefs = usePrefs()
const session = useSession()
const bot = computed(() => session.rules?.contact.qq_bot)
const account = useAccount()
const router = useRouter()
const themes: [Theme, string][] = [['system', '跟随系统'], ['light', '浅色'], ['dark', '深色']]
const docs = computed(() => {
  const d: [string, string][] = [['terms', '用户协议'], ['privacy', '隐私政策'], ['disclosure', '风险揭示书'], ['margin_disclosure', '融资融券风险揭示书'], ['futures_disclosure', '热度期货风险揭示书']]
  if (session.me?.api_trading?.enabled) d.push(['api_disclosure', '程序化交易风险揭示书'])
  return d
})
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

// ---- 注销账号 (docs/design/account.md §9): the password, then the account is gone for good
const closing = ref(false)
const closePw = ref('')
const closeErr = ref('')
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

// ---- API 密钥 (docs/design/api-access.md): read-only, or trading while the server allows it (身份验证 and the
// signed 程序化交易风险揭示书 first); the token is shown once
const trading = computed(() => session.me?.api_trading ?? null)
const keyTrade = ref(false)
const tradeReady = computed(() => !!trading.value?.verified && !!trading.value?.disclosure_signed)
const keys = ref<ApiKey[] | null>(null)
// an older server without API keys: hide the section
const keysMissing = ref(false)
const keyMax = ref(5)
const keyErr = ref('')
const making = ref(false)
const keyName = ref('')
const keyDays = ref(90)
const made = ref<ApiKeyCreated | null>(null)
const copied = ref(false)
const revokeId = ref<number | null>(null)
const lifetimes = [30, 90, 365]
const liveKeys = computed(() => (keys.value ?? []).filter((k) => !k.expired).length)
async function loadKeys() {
  try {
    const r = await call(api.GET('/api/v1/me/api-keys'))
    keys.value = r.items
    keyMax.value = r.max
  } catch (e) {
    if (e instanceof ApiError && e.code === 'not_found') keysMissing.value = true
    else keyErr.value = errorText(e)
  }
}
if (session.me) loadKeys()
function closeMaking() {
  making.value = false
  keyName.value = ''
  keyDays.value = 90
  keyTrade.value = false
  made.value = null
  copied.value = false
  keyErr.value = ''
}
async function makeKey() {
  busy.value = true
  keyErr.value = ''
  try {
    made.value = await call(api.POST('/api/v1/me/api-keys', { body: { name: keyName.value.trim(), expires_in_days: keyDays.value, scopes: keyTrade.value ? ['trade'] : ['read:account'] } }))
    await loadKeys()
  } catch (e) {
    keyErr.value = errorText(e)
  } finally {
    busy.value = false
  }
}
async function copyKey() {
  if (!made.value) return
  try {
    await navigator.clipboard.writeText(made.value.token)
    copied.value = true
  } catch {
    copied.value = false
  }
}
async function revokeKey(id: number) {
  busy.value = true
  keyErr.value = ''
  try {
    await call(api.DELETE('/api/v1/me/api-keys/{id}', { params: { path: { id } } }))
    revokeId.value = null
    await loadKeys()
  } catch (e) {
    keyErr.value = errorText(e)
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
          <button type="button" class="btn ghost small" @click="confirmId = binding.id">解除绑定</button>
        </div>
        <Modal :open="confirmId === binding.id" title="解除 QQ 绑定？" edit-key="unbind" :busy="busy" @close="confirmId = null">
          <p>身份验证随之取消，不能领取财政补贴、认购新股、进入排行或开通融资融券，也不能通过机器人找回密码。</p>
          <template #actions>
            <button type="button" class="btn" :disabled="busy" @click="unbind(binding.id)">解除绑定</button>
            <button type="button" class="btn ghost" :disabled="busy" @click="confirmId = null">取消</button>
          </template>
        </Modal>
      </template>
      <template v-else>
        <p class="hint">买卖股票不需要身份验证。以下功能需要身份验证：</p>
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
    <section v-if="session.me && !keysMissing" class="sec">
      <h2>API 密钥</h2>
      <p class="hint">{{ trading?.enabled ? '用于脚本读取账户数据、接收成交推送和下单。' : '用于脚本读取账户数据和接收成交推送，目前不能下单。' }}使用方法见<a href="https://github.com/HuanCheng65/anistock/blob/main/docs/api-scripts.md" target="_blank" rel="noopener">开发者说明</a>，接口定义见 <a href="/api/v1/openapi.json" target="_blank" rel="noopener">OpenAPI 文档</a>。</p>
      <ul v-if="keys?.length" class="keys">
        <li v-for="k in keys" :key="k.id">
          <div class="keyhead">
            <span class="keyname">{{ k.name }}</span>
            <button type="button" class="btn ghost small" @click="revokeId = k.id">吊销</button>
          </div>
          <div class="keymeta">
            <span class="num">{{ k.prefix }}…</span>
            <span v-if="k.scopes.includes('trade')">可交易</span>
            <span v-if="k.expired" class="expired">已过期</span>
            <span v-else>{{ k.expires_at.slice(0, 10) }} 到期</span>
            <span>{{ k.last_used_at ? `${when(k.last_used_at)}使用` : '未使用' }}</span>
          </div>
          <Modal :open="revokeId === k.id" title="吊销 API 密钥？" edit-key="revoke-key" :busy="busy" @close="revokeId = null">
            <p>使用“{{ k.name }}”的脚本将无法访问账户，吊销后不能恢复。</p>
            <template #actions>
              <button type="button" class="btn" :disabled="busy" @click="revokeKey(k.id)">吊销</button>
              <button type="button" class="btn ghost" :disabled="busy" @click="revokeId = null">取消</button>
            </template>
          </Modal>
        </li>
      </ul>
      <button type="button" class="btn small" :disabled="busy || liveKeys >= keyMax" @click="making = true">生成密钥</button>
      <p v-if="liveKeys >= keyMax" class="hint cap">最多同时有 {{ keyMax }} 个有效密钥</p>
      <p v-if="keyErr && !making" class="msg error" role="alert">{{ keyErr }}</p>
      <Modal :open="making" :title="made ? 'API 密钥' : '生成 API 密钥'" edit-key="make-key" :busy="busy" @close="closeMaking">
        <template v-if="made">
          <p>密钥只显示这一次，关闭后无法再查看。</p>
          <code class="token num">{{ made.token }}</code>
        </template>
        <form v-else id="key-form" class="fields" @submit.prevent="makeKey">
          <input autofocus v-model="keyName" class="input" maxlength="20" placeholder="名称，如“看盘脚本”" />
          <div class="opts" role="radiogroup" aria-label="有效期">
            <button v-for="d in lifetimes" :key="d" type="button" role="radio" :aria-checked="keyDays === d" @click="keyDays = d">{{ d }} 天</button>
          </div>
          <template v-if="trading?.enabled">
            <div class="opts" role="radiogroup" aria-label="权限">
              <button type="button" role="radio" :aria-checked="!keyTrade" @click="keyTrade = false">只读</button>
              <button type="button" role="radio" :aria-checked="keyTrade" @click="keyTrade = true">交易</button>
            </div>
            <p v-if="keyTrade && !trading.verified" class="hint">先完成上方的身份验证</p>
            <p v-if="keyTrade && !trading.disclosure_signed" class="hint">先签署<RouterLink to="/legal/api_disclosure?sign=1">《程序化交易风险揭示书》</RouterLink></p>
          </template>
          <p v-if="keyErr" class="msg error" role="alert">{{ keyErr }}</p>
        </form>
        <template #actions>
          <template v-if="made">
            <button type="button" class="btn" @click="copyKey">{{ copied ? '已复制' : '复制' }}</button>
            <button type="button" class="btn ghost" @click="closeMaking">完成</button>
          </template>
          <template v-else>
            <button type="submit" form="key-form" class="btn" :disabled="busy || !keyName.trim() || (keyTrade && !tradeReady)">生成</button>
            <button type="button" class="btn ghost" :disabled="busy" @click="closeMaking">取消</button>
          </template>
        </template>
      </Modal>
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
      <button type="button" class="linkbtn pw" @click="changing = true; pwMsg = null">修改密码</button>
      <p v-if="pwMsg?.kind === 'ok'" class="msg ok" role="status">{{ pwMsg.text }}</p>
      <Modal :open="changing" title="修改密码" edit-key="password" :busy="busy" @close="changing = false; curPw = ''; newPw = ''; pwMsg = null">
        <form id="pw-form" class="fields" @submit.prevent="changePassword">
          <input autofocus v-model="curPw" class="input" type="password" autocomplete="current-password" placeholder="当前密码" />
          <input v-model="newPw" class="input" type="password" autocomplete="new-password" placeholder="新密码，至少 6 位" />
          <p v-if="pwMsg?.kind === 'error'" class="msg error" role="alert">{{ pwMsg.text }}</p>
        </form>
        <template #actions>
          <button type="submit" form="pw-form" class="btn" :disabled="busy || !curPw || !newPw">修改密码</button>
          <button type="button" class="btn ghost" :disabled="busy" @click="changing = false; curPw = ''; newPw = ''; pwMsg = null">取消</button>
        </template>
      </Modal>
      <button type="button" class="linkbtn close" @click="closing = true">注销账号</button>
      <Modal :open="closing" title="注销账号？" edit-key="close" :busy="busy" @close="closing = false; closePw = ''; closeErr = ''">
        <form id="close-form" class="fields" @submit.prevent="closeAccount">
          <p>持仓按最新价转出，资金清零，个人信息删除。注销后账号无法恢复。</p>
          <input autofocus v-model="closePw" class="input" type="password" autocomplete="current-password" placeholder="输入密码确认" />
          <p v-if="closeErr" class="msg error" role="alert">{{ closeErr }}</p>
        </form>
        <template #actions>
          <button type="submit" form="close-form" class="btn danger" :disabled="busy || !closePw">注销账号</button>
          <button type="button" class="btn ghost" :disabled="busy" @click="closing = false; closePw = ''; closeErr = ''">取消</button>
        </template>
      </Modal>
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
.fields { display: flex; flex-direction: column; gap: 8px; }
.btn.danger { background: var(--danger); color: #fff; }
.switch { position: relative; width: 44px; height: 26px; flex: none; padding: 0; border: 1px solid var(--base-line); background: var(--field-bg); transition: background-color 0.2s ease; }
.switch span { position: absolute; top: 3px; left: 3px; width: 18px; height: 18px; background: var(--muted); transition: transform 0.2s cubic-bezier(0.2, 0.7, 0.2, 1), background-color 0.2s ease; }
.switch[aria-checked="true"] { background: var(--ink); border-color: var(--ink); }
.switch[aria-checked="true"] span { transform: translateX(18px); background: var(--ground); }
@media (prefers-reduced-motion: reduce) { .switch, .switch span { transition: none; } }
.docs { margin-top: -6px; }
.pw { display: block; margin-top: 14px; color: var(--ink-2); }
.close { display: block; margin-top: 14px; color: var(--muted); }
.fields .input { height: 44px; font-size: 16px; }
.docrow { display: flex; align-items: center; justify-content: space-between; padding: 12px 0; border-bottom: 1px solid var(--hair-2); font-size: 15px; }
.keys { list-style: none; margin: 0 0 12px; padding: 0; border-top: 1px solid var(--hair-2); }
.keys li { padding: 10px 0; border-bottom: 1px solid var(--hair-2); }
.keyhead { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.keyname { font-size: 15px; overflow-wrap: anywhere; }
.keymeta { display: flex; flex-wrap: wrap; gap: 4px 12px; margin-top: 2px; font-size: 12px; color: var(--muted); }
.keymeta .expired { color: var(--danger); }
.cap { margin: 8px 0 0; }
.token { display: block; margin-top: 10px; padding: 10px 12px; border: 1px solid var(--base-line); background: var(--field-bg); font-size: 13px; overflow-wrap: anywhere; user-select: all; }
.docrow:last-child { border-bottom: 0; }
.chev { color: var(--muted); }
</style>
