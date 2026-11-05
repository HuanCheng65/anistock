<script setup lang="ts">
import BrandMark from '../components/BrandMark.vue'
import { ref } from 'vue'
import { RouterLink, useRoute, useRouter } from 'vue-router'
import { ApiError } from '../api'
import { useSession } from '../stores/session'

const session = useSession()
const router = useRouter()
const route = useRoute()
const mode = ref<'login' | 'register'>('login')
const username = ref('')
const password = ref('')
const invite = ref('')
const busy = ref(false)
const error = ref('')

async function submit() {
  error.value = ''
  if (username.value.trim().length < 2) return (error.value = '用户名至少 2 个字')
  if (password.value.length < 6) return (error.value = '密码至少 6 位')
  if (mode.value === 'register' && !invite.value.trim()) return (error.value = '注册需要社团邀请码')
  busy.value = true
  try {
    if (mode.value === 'login') await session.login(username.value.trim(), password.value)
    else await session.register(username.value.trim(), password.value, invite.value.trim())
    router.replace((route.query.next as string) || (mode.value === 'register' ? '/guide' : '/'))
  } catch (e) {
    error.value = e instanceof ApiError ? e.message : '出错了，请重试'
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="page login">
    <section class="intro">
      <BrandMark :size="44" />
      <h1>AniEx 安易</h1>
      <p class="muted">每部新番都是一家上市公司。真实观众的热度就是它的营收，营收每周按持股分红。看好哪部番，就买它的股票。</p>
    </section>

    <form class="panel form" @submit.prevent="submit">
      <div class="seg" role="tablist">
        <button type="button" role="tab" :aria-pressed="mode === 'login'" @click="mode = 'login'">登录</button>
        <button type="button" role="tab" :aria-pressed="mode === 'register'" @click="mode = 'register'">注册</button>
      </div>
      <label class="field">
        <span class="label">用户名</span>
        <input id="login-username" class="input" v-model="username" autocomplete="username" maxlength="20" />
      </label>
      <label class="field">
        <span class="label">密码</span>
        <input id="login-password" class="input" type="password" v-model="password" :autocomplete="mode === 'login' ? 'current-password' : 'new-password'" />
      </label>
      <label class="field" v-if="mode === 'register'">
        <span class="label">邀请码（向社团管理员要）</span>
        <input id="login-invite" class="input" v-model="invite" autocomplete="off" />
      </label>
      <p v-if="error" class="notice error" role="alert">{{ error }}</p>
      <button class="btn primary block" :disabled="busy">{{ busy ? '请稍候…' : mode === 'login' ? '登录' : '注册并领取 10 万币' }}</button>
      <p class="small muted" v-if="mode === 'register'">注册后会得到 100,000 币的初始资金。游戏币不能充值，也不能兑换任何真实财物。</p>
    </form>
    <RouterLink to="/" class="small center">先随便看看行情 →</RouterLink>
  </main>
</template>

<style scoped>
.login { max-width: 420px; padding-top: 32px; }
.intro { display: flex; flex-direction: column; gap: 10px; padding: 8px 4px 4px; }
.intro h1 { font-size: 24px; font-weight: 700; }
.form { padding: 16px; display: flex; flex-direction: column; gap: 12px; }
.center { text-align: center; }
</style>
