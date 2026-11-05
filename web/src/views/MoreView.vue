<script setup lang="ts">
import { RouterLink, useRouter } from 'vue-router'
import { useSession } from '../stores/session'

const session = useSession()
const router = useRouter()
async function logout() {
  await session.logout()
  router.push('/')
}
</script>

<template>
  <main class="page">
    <nav class="panel menu" aria-label="更多">
      <RouterLink to="/ipo"><b>新股认购</b><span class="small muted">上市前 3 天认购，超额时按比例分配加抽签</span></RouterLink>
      <RouterLink to="/guide"><b>新手指南</b><span class="small muted">五分钟学会怎么玩</span></RouterLink>
      <RouterLink to="/rules"><b>规则</b><span class="small muted">交易、分红、税、上市退市的完整规则</span></RouterLink>
      <RouterLink to="/macro"><b>宏观</b><span class="small muted">央行利率和货币供应量</span></RouterLink>
    </nav>
    <section class="panel menu">
      <template v-if="session.me">
        <div class="row"><b>{{ session.me.username }}</b><span class="small muted">已登录</span></div>
        <button type="button" class="row danger" @click="logout">退出登录</button>
      </template>
      <RouterLink v-else to="/login"><b>登录 / 注册</b><span class="small muted">注册需要社团邀请码</span></RouterLink>
    </section>
    <p class="small muted foot">AniStock 是社团内部的模拟游戏，游戏币不能充值、不能兑换任何真实财物。番剧数据来自 AniList、Bangumi、Annict 的公开统计。</p>
  </main>
</template>

<style scoped>
.menu { display: flex; flex-direction: column; }
.menu a, .menu .row { display: flex; flex-direction: column; gap: 2px; padding: 14px 12px; border-bottom: 1px solid var(--rule); color: var(--ink); text-align: left; background: none; border-left: 0; border-right: 0; border-top: 0; }
.menu > :last-child { border-bottom: none; }
.danger { color: var(--up); cursor: pointer; font-weight: 600; }
.foot { padding: 0 4px; }
</style>
