<script setup lang="ts">
import { RouterLink, useRoute } from 'vue-router'
import { useSession } from '../stores/session'
import BrandMark from './BrandMark.vue'
import MarketClock from './MarketClock.vue'
import SearchPanel from './SearchPanel.vue'

const route = useRoute()
const session = useSession()
const nav = [
  { key: 'market', to: '/', label: '市场' },
  { key: 'stocks', to: '/stocks', label: '行情' },
  { key: 'ipo', to: '/ipo', label: '新股' },
  { key: 'news', to: '/news', label: '资讯' },
  { key: 'me', to: '/me', label: '我的' },
]
</script>

<template>
  <header class="desk">
    <div class="container bar">
      <RouterLink to="/" class="brand" aria-label="AniEx 首页">
        <BrandMark :size="26" />
        <span class="serif word">AniEx</span><span class="cn">安易</span>
      </RouterLink>
      <nav aria-label="主导航">
        <RouterLink v-for="n in nav" :key="n.key" :to="n.to" :aria-current="route.meta.tab === n.key ? 'page' : undefined">{{ n.label }}</RouterLink>
      </nav>
      <SearchPanel compact class="search" />
      <MarketClock />
      <RouterLink v-if="session.checked && !session.me" to="/login" class="who">登录</RouterLink>
      <RouterLink v-else-if="session.me" to="/me" class="who clamp1">{{ session.me.username }}</RouterLink>
    </div>
  </header>
</template>

<style scoped>
.desk { display: none; position: sticky; top: 0; z-index: 30; background: var(--ground); border-bottom: 1px solid var(--rule); }
@media (min-width: 1024px) { .desk { display: block; } }
.bar { height: var(--desk-header-h); display: flex; align-items: center; gap: 36px; }
.brand { display: flex; align-items: center; gap: 8px; color: var(--ink); flex: none; }
.word { font-weight: 900; font-size: 20px; }
.cn { font-size: 13px; color: var(--ink-2); padding-left: 8px; border-left: 1px solid var(--hair); }
nav { display: flex; gap: 28px; }
nav a { height: var(--desk-header-h); display: flex; align-items: center; padding: 0 2px; font-size: 15px; color: var(--ink-2); }
nav a[aria-current="page"] { color: var(--ink); font-weight: 700; box-shadow: inset 0 -2px 0 var(--ink); }
.search { margin-left: auto; width: 280px; flex: none; }
.who { font-size: 14px; max-width: 9em; color: var(--ink); flex: none; }
</style>
