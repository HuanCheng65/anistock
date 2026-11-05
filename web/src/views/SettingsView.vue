<script setup lang="ts">
import { useRouter } from 'vue-router'
import BackButton from '../components/BackButton.vue'
import { useAccount } from '../stores/account'
import { usePrefs, type Theme } from '../stores/prefs'
import { useSession } from '../stores/session'

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
</style>
