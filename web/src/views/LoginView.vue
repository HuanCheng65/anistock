<script setup lang="ts">
import { computed, ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { ApiError } from '../api'
import BrandMark from '../components/BrandMark.vue'
import Tabs from '../components/Tabs.vue'
import { usePrefs } from '../stores/prefs'
import { useSession } from '../stores/session'

const session = useSession()
const prefs = usePrefs()
const router = useRouter()
const route = useRoute()
const mode = ref(route.query.invite ? 'register' : 'login')
const username = ref('')
const password = ref('')
const invite = ref(typeof route.query.invite === 'string' ? route.query.invite : '')
const busy = ref(false)
const error = ref('')
const reg = computed(() => mode.value === 'register')
const tabs = [
  { key: 'login', label: '登录' },
  { key: 'register', label: '注册' },
]

async function submit() {
  error.value = ''
  const name = username.value.trim()
  if (name.length < 2 || name.length > 20) return (error.value = '用户名需为 2–20 个字符')
  if (password.value.length < 6) return (error.value = '密码至少 6 位')
  if (reg.value && !invite.value.trim()) return (error.value = '填写邀请码，可以向社团管理员索取')
  busy.value = true
  try {
    if (reg.value) await session.register(name, password.value, invite.value.trim())
    else await session.login(name, password.value)
    const next = typeof route.query.next === 'string' ? route.query.next : '/'
    if (!prefs.onboarded) router.replace({ name: 'welcome', query: { next } })
    else router.replace(next)
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '无法完成操作，稍后重试'
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
      <label v-if="reg" class="field">邀请码
        <input class="input" v-model="invite" autocomplete="off" />
      </label>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <button class="btn block" :disabled="busy">{{ busy ? (reg ? '注册中…' : '登录中…') : reg ? '注册' : '登录' }}</button>
    </form>

    <p class="foot">
      <template v-if="reg">注册后获得 100,000 币。</template>游戏币与真实货币无关，不能充值或提现。
      <RouterLink to="/" class="look">去看行情</RouterLink>
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
