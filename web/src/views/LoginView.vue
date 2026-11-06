<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { ApiError } from '../api'
import BrandMark from '../components/BrandMark.vue'
import Tabs from '../components/Tabs.vue'
import { powSupported, solve, type PowSolution } from '../pow'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

type Purpose = 'register' | 'login'

const session = useSession()
const prefs = usePrefs()
const router = useRouter()
const route = useRoute()
const mode = ref<Purpose>(route.query.invite ? 'register' : 'login')
const username = ref('')
const password = ref('')
const invite = ref(typeof route.query.invite === 'string' ? route.query.invite : '')
const busy = ref(false)
const error = ref('')
// open / invite / closed, from the server; unknown until the first challenge arrives
const registration = ref<string | null>(null)
const reg = computed(() => mode.value === 'register')
const needInvite = computed(() => registration.value === 'invite' || (registration.value === null && !!invite.value))
const closed = computed(() => reg.value && registration.value === 'closed')
const tabs = [
  { key: 'login', label: '登录' },
  { key: 'register', label: '注册' },
]

class Unsupported extends Error {}

// The proof of work is fetched and solved in the background while the form is being filled in.
let pending: { purpose: Purpose; answer: Promise<PowSolution | null> } | null = null

function prepare(purpose: Purpose) {
  const answer = (async () => {
    const r = await session.challenge(purpose)
    if (purpose === 'register') registration.value = r.registration
    if (!r.challenge) return null
    if (!powSupported()) throw new Unsupported()
    return solve(r.challenge)
  })()
  answer.catch(() => {}) // reported when the form is submitted
  pending = { purpose, answer }
}

function take(purpose: Purpose): Promise<PowSolution | null> {
  if (pending?.purpose !== purpose) prepare(purpose)
  const p = pending!.answer
  pending = null
  return p
}

prepare(mode.value)
watch(mode, (m) => {
  error.value = ''
  prepare(m)
})

async function attempt(name: string): Promise<void> {
  const purpose = mode.value
  const pow = await take(purpose)
  if (purpose === 'register') await session.register(name, password.value, needInvite.value ? invite.value.trim() : '', pow)
  else await session.login(name, password.value, pow)
}

async function submit() {
  error.value = ''
  const name = username.value.trim()
  if (name.length < 2 || name.length > 20) return (error.value = '用户名需为 2–20 个字符')
  if (password.value.length < 6) return (error.value = '密码至少 6 位')
  if (reg.value && needInvite.value && !invite.value.trim()) return (error.value = '填写邀请码，可以向社团管理员索取')
  busy.value = true
  try {
    try {
      await attempt(name)
    } catch (e) {
      // the challenge expired, or a surge started since it was fetched: once more with a fresh one
      if (e instanceof ApiError && (e.code === 'pow_required' || e.code === 'pow_invalid')) await attempt(name)
      else throw e
    }
    const next = typeof route.query.next === 'string' ? route.query.next : '/'
    if (!prefs.onboarded) router.replace({ name: 'welcome', query: { next } })
    else router.replace(next)
  } catch (e) {
    if (e instanceof Unsupported) error.value = '当前浏览器无法完成验证，在系统浏览器中打开后重试'
    else error.value = e instanceof ApiError ? e.message : '无法完成操作，稍后重试'
    prepare(mode.value) // a challenge is spent by its first try
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="login">
    <div class="brand">
      <BrandMark :size="64" />
      <div class="serif word">AniEx</div>
      <div class="serif cn">安易</div>
    </div>

    <Tabs v-model="mode" :items="tabs" label="账号" :size="17" class="tabs" />

    <form class="form" @submit.prevent="submit">
      <label class="field">用户名
        <input class="input" v-model="username" autocomplete="username" maxlength="20" :placeholder="reg ? '2–20 个字符' : ''" />
      </label>
      <label class="field">密码
        <input class="input" type="password" v-model="password" :autocomplete="reg ? 'new-password' : 'current-password'" :placeholder="reg ? '至少 6 位' : ''" />
      </label>
      <label v-if="reg && needInvite" class="field">邀请码
        <input class="input" v-model="invite" autocomplete="off" />
      </label>
      <p v-if="closed" class="msg">暂未开放注册</p>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <button class="btn block" :disabled="busy || closed">{{ busy ? (reg ? '注册中…' : '登录中…') : reg ? '注册' : '登录' }}</button>
    </form>

    <p class="foot">
      <template v-if="reg">注册后获得 100,000 星辉。</template>星辉是安易内的虚拟资产，与真实货币无关，不能充值或提现。
      <RouterLink to="/" class="look">去看市场</RouterLink>
    </p>
  </main>
</template>

<style scoped>
.login { min-height: 100vh; min-height: 100dvh; max-width: 420px; margin: 0 auto; padding: 0 24px; display: flex; flex-direction: column; }
.brand { display: flex; flex-direction: column; align-items: flex-start; gap: 14px; padding: calc(88px + env(safe-area-inset-top, 0px)) 0 36px; border-bottom: 1px solid var(--rule); }
.word { font-size: 34px; font-weight: 900; line-height: 1.1; }
.cn { font-size: 17px; font-weight: 600; color: var(--ink-2); }
.tabs { padding-top: 20px; border-bottom: 0; }
.form { display: flex; flex-direction: column; gap: 14px; padding-top: 18px; }
.form .btn { height: 48px; font-size: 16px; margin-top: 4px; }
.foot { margin: auto 0 28px; padding-top: 28px; font-size: 12px; line-height: 1.6; color: var(--muted); }
.look { margin-left: 4px; color: var(--ink-2); text-decoration: underline; text-underline-offset: 3px; }
@media (min-width: 1024px) { .brand { padding-top: 120px; } }
</style>
