<script setup lang="ts">
import { computed, onMounted, onBeforeUnmount } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
import { useSession } from './stores/session'

const route = useRoute()
const session = useSession()
const tab = computed(() => route.meta.tab as string | undefined)

const tabs = [
  { key: 'market', to: '/', label: '行情', icon: 'M3 17l5-6 4 3 7-9' },
  { key: 'me', to: '/me', label: '我的', icon: 'M4 7h16v12H4zM4 7l2-3h12l2 3M9 12h6' },
  { key: 'rank', to: '/rank', label: '排行', icon: 'M5 20V12M12 20V5M19 20v-6' },
  { key: 'news', to: '/news', label: '新闻', icon: 'M5 5h14v14H5zM8 9h8M8 13h8M8 17h5' },
  { key: 'more', to: '/more', label: '更多', icon: 'M5 12h.01M12 12h.01M19 12h.01' },
]

let timer: number | undefined
onMounted(() => {
  session.loadStatus()
  timer = window.setInterval(() => !document.hidden && session.loadStatus(), 30000)
})
onBeforeUnmount(() => window.clearInterval(timer))
</script>

<template>
  <header class="topbar">
    <div class="topbar-inner">
      <RouterLink to="/" class="brand" aria-label="AniStock 首页">
        <span class="seal" aria-hidden="true">番</span>
        <span class="word">AniStock</span>
      </RouterLink>
      <div class="state" v-if="session.status">
        <span class="dot" :class="session.status.trading ? 'open' : 'closed'"></span>
        <span>{{ session.status.trading ? '交易中' : '休市' }}</span>
        <span class="sep">·</span>
        <span>利率 <span class="num">{{ (session.status.rate * 100).toFixed(2) }}%</span></span>
      </div>
      <RouterLink v-if="session.checked && !session.me" to="/login" class="login-link">登录</RouterLink>
      <span v-else-if="session.me" class="who">{{ session.me.username }}</span>
    </div>
  </header>

  <RouterView v-slot="{ Component }">
    <component :is="Component" :key="route.path" />
  </RouterView>

  <nav class="tabbar" aria-label="主导航" v-if="route.name !== 'login'">
    <RouterLink v-for="t in tabs" :key="t.key" :to="t.to" class="tabitem" :aria-current="tab === t.key ? 'page' : undefined">
      <svg viewBox="0 0 24 24" width="22" height="22" aria-hidden="true">
        <path :d="t.icon" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
      </svg>
      <span>{{ t.label }}</span>
    </RouterLink>
  </nav>
</template>

<style scoped>
.topbar {
  position: sticky; top: 0; z-index: 20;
  background: var(--surface); border-bottom: 1px solid var(--rule);
  padding-top: env(safe-area-inset-top, 0px);
}
.topbar-inner {
  max-width: 1120px; margin: 0 auto; height: 48px;
  padding-left: max(12px, env(safe-area-inset-left, 0px));
  padding-right: max(12px, env(safe-area-inset-right, 0px));
  display: flex; align-items: center; gap: 12px;
}
.brand { display: flex; align-items: center; gap: 8px; color: var(--ink); }
.seal {
  width: 26px; height: 26px; display: grid; place-items: center;
  background: var(--seal); color: #fff; border-radius: 3px;
  font-size: 15px; font-weight: 700; font-family: "Songti SC", "SimSun", "Noto Serif SC", serif;
}
.word { font-weight: 700; letter-spacing: 0.02em; font-size: 16px; }
.state { margin-left: auto; display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--ink-2); }
.sep { color: var(--rule-strong); }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.dot.open { background: var(--up); box-shadow: 0 0 0 3px var(--up-bg); }
.login-link { font-size: 14px; font-weight: 500; }
.who { font-size: 13px; color: var(--ink-2); max-width: 7em; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

.tabbar {
  position: fixed; left: 0; right: 0; bottom: 0; z-index: 20;
  display: flex; justify-content: center;
  background: var(--surface); border-top: 1px solid var(--rule);
  padding-bottom: env(safe-area-inset-bottom, 0px);
  padding-left: env(safe-area-inset-left, 0px);
  padding-right: env(safe-area-inset-right, 0px);
}
.tabitem {
  flex: 1; max-width: 140px; height: var(--tab-h);
  display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 2px;
  color: var(--muted); font-size: 11px;
}
.tabitem[aria-current="page"] { color: var(--ink); font-weight: 600; }
.tabitem[aria-current="page"] svg { color: var(--seal); }
</style>
