<script setup lang="ts">
// 找回密码 (docs/v2-account.md §13): a code from the QQ bot (private message, /找回密码), or from the
// operator for an account without 身份验证.
import { ref } from 'vue'
import { RouterLink, useRouter } from 'vue-router'
import { api, call, errorText } from '../api'
import BackButton from '../components/BackButton.vue'
import { useSession } from '../stores/session'

const session = useSession()
const router = useRouter()
const username = ref('')
const code = ref('')
const password = ref('')
const busy = ref(false)
const error = ref('')

async function submit() {
  error.value = ''
  if (!username.value.trim()) return (error.value = '填写用户名')
  if (code.value.replace(/[-\s]/g, '').length !== 8) return (error.value = '验证码是 8 位字母和数字')
  if (password.value.length < 6) return (error.value = '新密码至少 6 位')
  busy.value = true
  try {
    session.me = await call(api.POST('/api/v1/auth/password-reset', {
      body: { username: username.value.trim(), code: code.value.trim(), new_password: password.value },
    }))
    session.checked = true
    if (session.me?.pending_documents.length) router.replace({ name: 'welcome', query: { next: '/' } })
    else router.replace('/')
  } catch (e) {
    error.value = errorText(e)
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <main class="forgot">
    <header class="head">
      <BackButton fallback="/login" />
      <h1 class="serif">找回密码</h1>
    </header>
    <p class="lede">在 QQ 里私聊机器人，发送“/找回密码”，获取 8 位验证码。</p>
    <form class="form" @submit.prevent="submit">
      <label class="field">用户名
        <input v-model="username" class="input" autocomplete="username" maxlength="20" />
      </label>
      <label class="field">验证码
        <input v-model="code" class="input num" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" maxlength="9" placeholder="K7Q3-9MZD" />
      </label>
      <label class="field">新密码
        <input v-model="password" class="input" type="password" autocomplete="new-password" placeholder="至少 6 位" />
      </label>
      <p v-if="error" class="msg error" role="alert">{{ error }}</p>
      <button class="btn block" :disabled="busy">{{ busy ? '处理中…' : '重设密码' }}</button>
    </form>
    <p class="foot">没有完成身份验证的账号，发邮件到 aniex@huanchengfly.top 联系运营者。<RouterLink to="/login" class="back">返回登录</RouterLink></p>
  </main>
</template>

<style scoped>
.forgot { min-height: 100vh; min-height: 100dvh; max-width: 420px; margin: 0 auto; padding: 0 24px; display: flex; flex-direction: column; }
.head { display: flex; align-items: center; gap: 4px; margin-left: -12px; padding-top: calc(16px + env(safe-area-inset-top, 0px)); }
.head h1 { font-size: 26px; font-weight: 900; }
.lede { padding: 14px 0 6px; font-size: 14px; line-height: 1.7; color: var(--ink-2); text-wrap: pretty; }
.form { display: flex; flex-direction: column; gap: 14px; padding-top: 12px; }
.form .btn { height: 48px; font-size: 16px; margin-top: 4px; }
.foot { margin: auto 0 28px; padding-top: 28px; font-size: 12px; line-height: 1.6; color: var(--muted); }
.back { margin-left: 4px; color: var(--ink-2); text-decoration: underline; text-underline-offset: 3px; }
</style>
